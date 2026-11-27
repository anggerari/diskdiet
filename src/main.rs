mod docker;
mod picker;
mod scan;
mod service;
mod targets;
mod util;

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use util::{free_space, human_bytes, parse_duration, parse_size, pretty_path};

/// Put your disk on a diet 🍔➡️🥗
///
/// Finds and removes node_modules, build artifacts, package-manager caches
/// and Docker leftovers.
#[derive(Parser)]
#[command(name = "diskdiet", version, about, long_about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show what can be cleaned (read-only)
    Scan(Filters),
    /// Pick what to delete and delete it
    Clean(CleanArgs),
    /// Show Docker disk usage and prune it
    Docker(DockerArgs),
    /// Run diskdiet automatically in the background (launchd / systemd)
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

#[derive(Args, Clone)]
struct Filters {
    /// Folders to search for project artifacts [default: current folder]
    paths: Vec<PathBuf>,
    /// Only include projects untouched for at least this long (e.g. 30d, 2w)
    #[arg(long, value_parser = parse_duration)]
    older_than: Option<Duration>,
    /// Ignore anything smaller than this (e.g. 50MB)
    #[arg(long, value_parser = parse_size, default_value = "1MB")]
    min_size: u64,
    /// Skip project folders (node_modules, target, ...)
    #[arg(long)]
    no_projects: bool,
    /// Skip global caches (npm, cargo, Xcode, ...)
    #[arg(long)]
    no_caches: bool,
    /// Skip Docker
    #[arg(long)]
    no_docker: bool,
}

#[derive(Args)]
struct CleanArgs {
    #[command(flatten)]
    filters: Filters,
    /// Don't ask: delete everything found (used by the background service)
    #[arg(short, long)]
    yes: bool,
    /// Only show what would be deleted
    #[arg(long)]
    dry_run: bool,
    /// Docker: also remove all unused images (not just dangling) and all build cache
    #[arg(long)]
    docker_all: bool,
    /// Docker: also remove unused volumes (may delete database data!)
    #[arg(long)]
    docker_volumes: bool,
}

#[derive(Args)]
struct DockerArgs {
    /// Skip the picker and bulk-prune like `docker system prune`
    #[arg(long)]
    prune: bool,
    /// With --prune/--yes: also remove all unused images (not just dangling) and all build cache
    #[arg(long)]
    all: bool,
    /// Also remove unused volumes (may delete database data!)
    #[arg(long)]
    volumes: bool,
    /// Don't ask for confirmation
    #[arg(short, long)]
    yes: bool,
    /// Only show what would be deleted
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Install a scheduled job that cleans old project artifacts and caches
    Install {
        /// Folders to search [default: your home folder]
        paths: Vec<PathBuf>,
        /// How often to run
        #[arg(long, default_value = "7d", value_parser = parse_duration)]
        every: Duration,
        /// Only clean projects untouched for this long
        #[arg(long, default_value = "14d")]
        older_than: String,
        /// Also prune Docker (stopped containers, dangling images, build cache)
        #[arg(long)]
        with_docker: bool,
        /// Leave global caches alone
        #[arg(long)]
        no_caches: bool,
    },
    /// Remove the scheduled job
    Uninstall,
    /// Show whether the scheduled job is installed
    Status,
}

enum What {
    Artifact {
        path: PathBuf,
        kind: &'static str,
        modified: Option<SystemTime>,
    },
    Cache {
        name: &'static str,
        paths: Vec<PathBuf>,
    },
    Docker(docker::Obj),
}

struct Item {
    what: What,
    size: u64,
}

impl Item {
    fn default_selected(&self) -> bool {
        match &self.what {
            // Docker prune and emptying the Trash are opt-in.
            What::Docker(o) => o.default_selected(),
            What::Cache { name, .. } => *name != "Trash",
            What::Artifact { .. } => true,
        }
    }

    /// What `clean --yes` removes.
    fn auto_selected(&self, a: &CleanArgs) -> bool {
        match &self.what {
            What::Docker(o) => o.auto_selected(a.docker_all, a.docker_volumes),
            _ => true,
        }
    }

    fn row(&self, home: &Path) -> picker::Row {
        let (kind, age, label) = match &self.what {
            What::Artifact {
                path,
                kind,
                modified,
            } => (kind.to_string(), age(*modified), pretty_path(path, home)),
            What::Cache { name, paths } => (
                "cache".into(),
                "-".into(),
                format!(
                    "{name} {}",
                    style(
                        paths
                            .iter()
                            .map(|p| pretty_path(p, home))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                    .dim()
                ),
            ),
            What::Docker(o) => (o.kind_label().into(), o.age.clone(), o.label.clone()),
        };
        let size_label = human_bytes(self.size);
        picker::Row {
            kind,
            age,
            size: self.size,
            size_label,
            label,
            selected: self.default_selected(),
        }
    }
}

fn age(t: Option<SystemTime>) -> String {
    let Some(d) = t.and_then(|t| t.elapsed().ok()) else {
        return "?".into();
    };
    let days = d.as_secs() / 86_400;
    match days {
        0 => "today".into(),
        d if d < 365 => format!("{d}d"),
        d => format!("{}y", d / 365),
    }
}

fn spinner(msg: &str) -> ProgressBar {
    let pb = if io::stderr().is_terminal() {
        ProgressBar::new_spinner()
    } else {
        ProgressBar::hidden()
    };
    pb.set_style(ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message(msg.to_string());
    pb
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("could not determine home directory")
}

fn gather(f: &Filters, home: &Path) -> Result<Vec<Item>> {
    let pb = spinner("Scanning…");
    let mut items = Vec::new();

    if !f.no_projects {
        let roots: Vec<PathBuf> = if f.paths.is_empty() {
            vec![std::env::current_dir()?]
        } else {
            f.paths
                .iter()
                .map(|p| {
                    p.canonicalize()
                        .with_context(|| format!("path not found: {}", p.display()))
                })
                .collect::<Result<_>>()?
        };
        let found = scan::find_artifacts(&roots, home, &pb);
        pb.set_message(format!("Measuring {} folders…", found.len()));
        for a in scan::measure(found) {
            if let Some(min_age) = f.older_than {
                let old_enough = a
                    .last_modified
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|e| e >= min_age);
                if !old_enough {
                    continue;
                }
            }
            items.push(Item {
                size: a.size,
                what: What::Artifact {
                    path: a.path,
                    kind: a.kind,
                    modified: a.last_modified,
                },
            });
        }
    }

    if !f.no_caches {
        pb.set_message("Measuring caches…");
        for c in scan::find_caches(home) {
            items.push(Item {
                size: c.size,
                what: What::Cache {
                    name: c.name,
                    paths: c.paths,
                },
            });
        }
    }

    if !f.no_docker {
        pb.set_message("Asking Docker…");
        if docker::available() {
            match docker::scan() {
                Ok(objs) => items.extend(objs.into_iter().map(|o| Item {
                    size: o.size,
                    what: What::Docker(o),
                })),
                Err(e) => eprintln!("  {} {e:#}", style("warning:").yellow()),
            }
        }
    }
    pb.finish_and_clear();

    // Stopped containers are tiny but block image removal, so always show them.
    items.retain(|i| {
        i.size >= f.min_size
            || matches!(&i.what, What::Docker(o) if o.kind == docker::Kind::Container)
    });
    items.sort_by_key(|i| std::cmp::Reverse(i.size));
    Ok(items)
}

fn header(title: &str, home: &Path, items: &[Item]) {
    let total: u64 = items.iter().map(|i| i.size).sum();
    println!();
    println!("{}", style(format!("diskdiet {title}")).bold());
    let free = free_space(home)
        .map(human_bytes)
        .unwrap_or_else(|| "?".into());
    println!(
        "{}",
        style(format!(
            "{free} free on disk. Found {} item(s), ~{} reclaimable.",
            items.len(),
            human_bytes(total)
        ))
        .dim()
    );
    println!();
}

fn cmd_scan(f: Filters) -> Result<()> {
    let home = home()?;
    let items = gather(&f, &home)?;
    header("scan", &home, &items);
    if items.is_empty() {
        println!("Nothing to clean. Your disk is already lean 🥗");
        return Ok(());
    }
    let mut rows: Vec<_> = items.iter().map(|i| i.row(&home)).collect();
    rows.iter_mut().for_each(|r| r.selected = false);
    for (i, r) in rows.iter().enumerate() {
        println!(
            "  {:>3}  {:<12} {:>6}  {:>9}  {}",
            i + 1,
            r.kind,
            r.age,
            r.size_label,
            r.label
        );
    }
    println!();
    println!(
        "Run {} to choose what to delete.",
        style("diskdiet clean").cyan()
    );
    if !f.no_docker && items.iter().any(|i| matches!(i.what, What::Docker(_))) {
        if let Ok(rows) = docker::usage() {
            println!();
            println!("{}", style("Docker summary").bold());
            for r in rows {
                println!(
                    "  {:<15} {:>10}  reclaimable {}",
                    r.kind, r.size, r.reclaimable
                );
            }
        }
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    let term = console::Term::stdout();
    write!(&term, "  {prompt} [y/N]: ")?;
    term.flush()?;
    let s = term.read_line()?;
    Ok(matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn cmd_clean(a: CleanArgs) -> Result<()> {
    let home = home()?;
    let started = SystemTime::now();
    if a.yes {
        let ts = started
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        println!("diskdiet clean run (unix time {ts})");
    }
    let items = gather(&a.filters, &home)?;
    header("clean", &home, &items);
    if items.is_empty() {
        println!("Nothing to clean. Your disk is already lean 🥗");
        return Ok(());
    }

    let mut rows: Vec<_> = items.iter().map(|i| i.row(&home)).collect();
    let selected: Vec<&Item> = if a.yes || a.dry_run {
        for (row, item) in rows.iter_mut().zip(&items) {
            row.selected = item.auto_selected(&a);
        }
        for (i, r) in rows.iter().enumerate().filter(|(_, r)| r.selected) {
            println!(
                "  {:>3}  {:<12} {:>6}  {:>9}  {}",
                i + 1,
                r.kind,
                r.age,
                r.size_label,
                r.label
            );
        }
        println!();
        if a.dry_run {
            println!("Dry run: nothing was deleted.");
            return Ok(());
        }
        items.iter().filter(|i| i.auto_selected(&a)).collect()
    } else {
        if !io::stdin().is_terminal() {
            bail!("not running in a terminal; pass --yes to clean without asking or --dry-run to preview");
        }
        if picker::pick(&mut rows)?.is_none() {
            println!("Cancelled. Nothing was deleted.");
            return Ok(());
        }
        let chosen: Vec<&Item> = items
            .iter()
            .zip(&rows)
            .filter(|(_, r)| r.selected)
            .map(|(i, _)| i)
            .collect();
        if chosen.is_empty() {
            println!("Nothing selected. Nothing was deleted.");
            return Ok(());
        }
        let bytes: u64 = chosen.iter().map(|i| i.size).sum();
        println!();
        if !confirm(&format!(
            "Delete {} item(s), ~{}?",
            chosen.len(),
            human_bytes(bytes)
        ))? {
            println!("Cancelled. Nothing was deleted.");
            return Ok(());
        }
        chosen
    };

    let before = free_space(&home);
    let (fs_items, mut docker_selected): (Vec<&Item>, Vec<&Item>) = selected
        .into_iter()
        .partition(|i| !matches!(i.what, What::Docker(_)));

    let pb = if io::stderr().is_terminal() {
        ProgressBar::new(fs_items.len() as u64)
    } else {
        ProgressBar::hidden()
    };
    pb.set_style(ProgressStyle::with_template("  {bar:30.green/dim} {pos}/{len} {msg}").unwrap());
    let freed = AtomicU64::new(0);
    let errors: Vec<String> = fs_items
        .par_iter()
        .filter_map(|item| {
            let res = match &item.what {
                What::Artifact { path, .. } => fs::remove_dir_all(path)
                    .map_err(|e| format!("{}: {e}", pretty_path(path, &home))),
                What::Cache { paths, name } => paths
                    .iter()
                    .map(|p| {
                        scan::clear_dir_contents(p)
                            .map_err(|e| format!("{name} ({}): {e}", pretty_path(p, &home)))
                    })
                    .find(|r| r.is_err())
                    .unwrap_or(Ok(())),
                What::Docker(_) => Ok(()),
            };
            pb.inc(1);
            match res {
                Ok(()) => {
                    freed.fetch_add(item.size, Ordering::Relaxed);
                    None
                }
                Err(e) => Some(e),
            }
        })
        .collect();
    pb.finish_and_clear();

    for e in &errors {
        eprintln!("  {} {e}", style("warning:").yellow());
    }

    // Docker: containers first, then images, volumes, build cache.
    docker_selected.sort_by_key(|i| match &i.what {
        What::Docker(o) => o.kind,
        _ => docker::Kind::BuildCache,
    });
    for item in docker_selected {
        let What::Docker(o) = &item.what else {
            continue;
        };
        match o.delete() {
            Ok(()) => {
                freed.fetch_add(item.size, Ordering::Relaxed);
            }
            Err(e) => eprintln!("  {} {}: {e:#}", style("warning:").yellow(), o.label),
        }
    }

    println!();
    println!(
        "  {} freed ~{}",
        style("Done!").green().bold(),
        style(human_bytes(freed.into_inner())).bold()
    );
    if let (Some(b), Some(after)) = (before, free_space(&home)) {
        println!(
            "  Free space: {} → {}",
            human_bytes(b),
            style(human_bytes(after)).green().bold()
        );
    }
    if a.yes {
        if let Ok(d) = started.elapsed() {
            println!("  Took {:.1}s", d.as_secs_f64());
        }
    }
    Ok(())
}

fn cmd_docker(a: DockerArgs) -> Result<()> {
    if !docker::available() {
        bail!("Docker is not running (or the docker CLI is not installed)");
    }
    if !a.prune {
        // Itemized: every image, stopped container, unused volume and build cache.
        return cmd_clean(CleanArgs {
            filters: Filters {
                paths: vec![],
                older_than: None,
                min_size: 0,
                no_projects: true,
                no_caches: true,
                no_docker: false,
            },
            yes: a.yes,
            dry_run: a.dry_run,
            docker_all: a.all,
            docker_volumes: a.volumes,
        });
    }
    let rows = docker::usage()?;
    println!();
    println!("{}", style("Docker disk usage").bold());
    for r in &rows {
        println!(
            "  {:<15} {:>10}  reclaimable {}",
            r.kind, r.size, r.reclaimable
        );
    }
    println!();
    let what = match (a.all, a.volumes) {
        (false, false) => "stopped containers, dangling images, unused networks, dangling build cache",
        (true, false) => "stopped containers, ALL unused images, unused networks, ALL build cache",
        (false, true) => "stopped containers, dangling images, unused networks, dangling build cache, UNUSED VOLUMES",
        (true, true) => "stopped containers, ALL unused images, unused networks, ALL build cache, UNUSED VOLUMES",
    };
    println!("  Will remove: {what}");
    if !a.yes && !confirm("Continue?")? {
        println!("Cancelled.");
        return Ok(());
    }
    docker::prune(a.all, a.volumes)?;
    if cfg!(target_os = "macos") {
        println!();
        println!(
            "{}",
            style("Tip: Docker Desktop's disk image may take a moment to shrink. If it doesn't, use Docker Desktop → Troubleshoot → Clean / Purge data.").dim()
        );
    }
    Ok(())
}

fn cmd_service(action: ServiceCmd) -> Result<()> {
    let home = home()?;
    match action {
        ServiceCmd::Install {
            paths,
            every,
            older_than,
            with_docker,
            no_caches,
        } => {
            parse_duration(&older_than).map_err(anyhow::Error::msg)?;
            let paths = if paths.is_empty() {
                vec![home.clone()]
            } else {
                paths
            };
            let mut args = vec![
                "clean".to_string(),
                "--yes".into(),
                "--older-than".into(),
                older_than,
            ];
            if !with_docker {
                args.push("--no-docker".into());
            }
            if no_caches {
                args.push("--no-caches".into());
            }
            for p in paths {
                let p = p
                    .canonicalize()
                    .with_context(|| format!("path not found: {}", p.display()))?;
                args.push(p.to_string_lossy().into_owned());
            }
            service::install(&home, &service::ServiceConfig { every, args })
        }
        ServiceCmd::Uninstall => service::uninstall(&home),
        ServiceCmd::Status => service::status(&home),
    }
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Scan(f) => cmd_scan(f),
        Cmd::Clean(a) => cmd_clean(a),
        Cmd::Docker(a) => cmd_docker(a),
        Cmd::Service { action } => cmd_service(action),
    };
    if let Err(e) = res {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}
