mod picker;
mod tui;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use diskdiet::config::{self, Config};
use diskdiet::engine::{self, AutoPolicy, DeleteOptions, Item, Options};
use diskdiet::util::{free_space, human_bytes, now_secs, parse_duration, parse_size};
use diskdiet::{alert, docker, service, stats};

/// Put your storage on a diet 🍔➡️🥗 (beta)
///
/// Finds and removes node_modules, build artifacts, package-manager caches,
/// Docker leftovers and big forgotten files.
#[derive(Parser)]
#[command(name = "diskdiet", version, about, long_about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show what can be cleaned (read-only)
    Scan {
        #[command(flatten)]
        filters: Filters,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Pick what to delete and delete it
    Clean {
        #[command(flatten)]
        filters: Filters,
        #[command(flatten)]
        clean: CleanFlags,
    },
    /// Find big forgotten files (installers, archives, videos, iOS backups)
    Big {
        /// Folders to search [default: Downloads, Desktop, Documents, Movies]
        paths: Vec<PathBuf>,
        /// Only files at least this big [default: 500MB]
        #[arg(long, value_parser = parse_size)]
        min_size: Option<u64>,
        #[command(flatten)]
        clean: CleanFlags,
    },
    /// Pick Docker images / containers / volumes to remove, or bulk-prune
    Docker {
        /// Skip the picker and bulk-prune like `docker system prune`
        #[arg(long)]
        prune: bool,
        #[command(flatten)]
        clean: CleanFlags,
    },
    /// Notify when free space drops below a threshold
    Alert {
        /// Threshold, e.g. 20GB
        #[arg(long, default_value = "20GB", value_parser = parse_size)]
        below: u64,
        /// Print only; don't show a desktop notification
        #[arg(long)]
        no_notify: bool,
    },
    /// Show how much space diskdiet has freed so far
    Stats {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage ~/.config/diskdiet/config.toml
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Run diskdiet automatically in the background (launchd / systemd)
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

#[derive(Args, Clone)]
struct Filters {
    /// Folders to search for project artifacts [default: config `paths`, else current folder]
    paths: Vec<PathBuf>,
    /// Only include projects untouched for at least this long (e.g. 30d, 2w)
    #[arg(long, value_parser = parse_duration)]
    older_than: Option<Duration>,
    /// Ignore anything smaller than this (e.g. 50MB) [default: 1MB]
    #[arg(long, value_parser = parse_size)]
    min_size: Option<u64>,
    /// Skip project folders (node_modules, target, ...)
    #[arg(long)]
    no_projects: bool,
    /// Skip global caches (npm, cargo, Xcode, ...)
    #[arg(long)]
    no_caches: bool,
    /// Skip Docker
    #[arg(long)]
    no_docker: bool,
    /// Also look for big files in Downloads, Desktop, Documents and Movies
    #[arg(long)]
    big: bool,
    /// Don't check git status of projects
    #[arg(long)]
    no_git: bool,
}

#[derive(Args, Clone, Default)]
struct CleanFlags {
    /// Don't ask: delete everything that is safe to delete automatically
    #[arg(short, long)]
    yes: bool,
    /// Only show what `--yes` would delete
    #[arg(long)]
    dry_run: bool,
    /// Move to Trash instead of deleting permanently
    #[arg(long)]
    trash: bool,
    /// Full-screen picker (arrow keys + space)
    #[arg(long)]
    tui: bool,
    /// Wipe caches even when a smarter prune command exists
    #[arg(long)]
    wipe_caches: bool,
    /// Docker (with --yes): also remove tagged images no container uses
    #[arg(long)]
    docker_all: bool,
    /// Docker (with --yes): also remove unused volumes (may delete database data!)
    #[arg(long)]
    docker_volumes: bool,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Create a commented config file
    Init,
    /// Print the config file location
    Path,
    /// Print the current config file
    Show,
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Install scheduled jobs: periodic cleanup and (optionally) a low-space alert
    Install {
        /// Folders to search [default: your home folder]
        paths: Vec<PathBuf>,
        /// How often to clean
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
        /// Move to Trash instead of deleting
        #[arg(long)]
        trash: bool,
        /// Also notify when free space drops below this (e.g. 20GB)
        #[arg(long)]
        alert_below: Option<String>,
        /// How often to check free space
        #[arg(long, default_value = "1h", value_parser = parse_duration)]
        alert_every: Duration,
    },
    /// Remove the scheduled jobs
    Uninstall,
    /// Show whether the scheduled jobs are installed
    Status,
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("could not determine home directory")
}

fn spinner() -> ProgressBar {
    let pb = if io::stderr().is_terminal() {
        ProgressBar::new_spinner()
    } else {
        ProgressBar::hidden()
    };
    pb.set_style(ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message("Scanning…");
    pb
}

impl Filters {
    fn options(&self, cfg: &Config, home: &Path) -> Result<Options> {
        let mut o = Options::from_config(cfg, home);
        if !self.paths.is_empty() {
            o.paths = self.paths.clone();
        }
        if o.paths.is_empty() {
            o.paths = vec![std::env::current_dir()?];
        }
        for p in &o.paths {
            if !p.exists() {
                bail!("path not found: {}", p.display());
            }
        }
        if self.older_than.is_some() {
            o.older_than = self.older_than;
        }
        if let Some(m) = self.min_size {
            o.min_size = m;
        }
        o.projects = !self.no_projects;
        o.caches = !self.no_caches;
        o.docker = !self.no_docker;
        o.big = self.big;
        o.git = !self.no_git;
        Ok(o)
    }
}

fn gather(opts: &Options, cfg: &Config, home: &Path) -> Vec<Item> {
    let pb = spinner();
    let scan = engine::gather(opts, cfg, home, &|m| pb.set_message(m.to_string()));
    pb.finish_and_clear();
    for w in scan.warnings {
        eprintln!("  {} {w}", style("warning:").yellow());
    }
    scan.items
}

fn smart(cfg: &Config, flags: &CleanFlags) -> bool {
    !flags.wipe_caches && cfg.smart_caches.unwrap_or(true)
}

fn rows(items: &[Item], home: &Path, smart: bool) -> Vec<picker::Row> {
    items
        .iter()
        .map(|i| picker::Row {
            kind: i.kind_label(),
            age: i.age_label(),
            size: i.size,
            size_label: human_bytes(i.size),
            label: i.label(home),
            note: i.note(smart),
            selected: i.default_selected(),
        })
        .collect()
}

fn header(title: &str, home: &Path, items: &[Item]) {
    let total: u64 = items.iter().map(|i| i.size).sum();
    println!();
    println!(
        "{} {}",
        style(format!("diskdiet {title}")).bold(),
        style(" beta ").black().on_yellow()
    );
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

fn print_list(rows: &[picker::Row], only_selected: bool) {
    for (i, r) in rows.iter().enumerate() {
        if only_selected && !r.selected {
            continue;
        }
        println!(
            "  {:>3}  {:<13} {:>6}  {:>10}  {}{}",
            i + 1,
            r.kind,
            r.age,
            r.size_label,
            r.label,
            picker::note(r)
        );
    }
}

fn cmd_scan(filters: Filters, json: bool) -> Result<()> {
    let home = home()?;
    let cfg = Config::load(&home)?;
    let opts = filters.options(&cfg, &home)?;
    let items = gather(&opts, &cfg, &home);
    let smart = cfg.smart_caches.unwrap_or(true);
    if json {
        let out = serde_json::json!({
            "version": diskdiet::VERSION,
            "free_space": free_space(&home),
            "reclaimable": items.iter().map(|i| i.size).sum::<u64>(),
            "items": items.iter().map(|i| i.to_json(&home, smart)).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    header("scan", &home, &items);
    if items.is_empty() {
        println!("Nothing to clean. Your storage is already lean 🥗");
        return Ok(());
    }
    print_list(&rows(&items, &home, smart), false);
    println!();
    println!(
        "Run {} to choose what to delete.",
        style("diskdiet clean").cyan()
    );
    if opts.docker
        && items
            .iter()
            .any(|i| i.category() == engine::Category::Docker)
    {
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

fn run_clean(title: &str, opts: Options, cfg: &Config, home: &Path, f: CleanFlags) -> Result<()> {
    let started = Instant::now();
    if f.yes {
        println!(
            "diskdiet {} run at unix time {}",
            diskdiet::VERSION,
            now_secs()
        );
    }
    let smart = smart(cfg, &f);
    let trash = f.trash || cfg.trash;
    let items = gather(&opts, cfg, home);
    header(title, home, &items);
    if items.is_empty() {
        println!("Nothing to clean. Your storage is already lean 🥗");
        return Ok(());
    }

    let policy = AutoPolicy {
        docker_all: f.docker_all,
        docker_volumes: f.docker_volumes,
    };
    let mut rows = rows(&items, home, smart);
    let chosen: Vec<&Item> = if f.yes || f.dry_run {
        for (row, item) in rows.iter_mut().zip(&items) {
            row.selected = item.auto_selected(policy);
        }
        print_list(&rows, true);
        if !rows.iter().any(|r| r.selected) {
            println!("  Nothing here is removed automatically (big files, Trash, risky Docker items and dirty projects always need you to pick them).");
        }
        println!();
        if f.dry_run {
            println!("Dry run: nothing was deleted.");
            return Ok(());
        }
        items.iter().filter(|i| i.auto_selected(policy)).collect()
    } else {
        if !io::stdin().is_terminal() {
            bail!("not running in a terminal; pass --yes to clean without asking or --dry-run to preview");
        }
        let use_tui = f.tui || cfg.picker.as_deref() == Some("tui");
        let picked = if use_tui {
            tui::pick(&mut rows, &format!("diskdiet {title} (beta)"))?
        } else {
            picker::pick(&mut rows)?
        };
        if picked.is_none() {
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
        let verb = if trash { "Move to Trash" } else { "Delete" };
        if !confirm(&format!(
            "{verb} {} item(s), ~{}?",
            chosen.len(),
            human_bytes(bytes)
        ))? {
            println!("Cancelled. Nothing was deleted.");
            return Ok(());
        }
        chosen
    };

    let before = free_space(home);
    let pb = if io::stderr().is_terminal() {
        ProgressBar::new(chosen.len() as u64)
    } else {
        ProgressBar::hidden()
    };
    pb.set_style(ProgressStyle::with_template("  {bar:30.green/dim} {pos}/{len}").unwrap());
    let outcome = engine::delete(
        &chosen,
        home,
        DeleteOptions {
            trash,
            smart_caches: smart,
        },
        &|done, _| pb.set_position(done as u64),
    );
    pb.finish_and_clear();

    for e in &outcome.errors {
        eprintln!("  {} {e}", style("warning:").yellow());
    }
    let _ = stats::record(
        outcome.freed,
        outcome.trashed,
        outcome.removed as u64,
        if f.yes { "auto" } else { "cli" },
    );

    println!();
    println!(
        "  {} freed ~{}",
        style("Done!").green().bold(),
        style(human_bytes(outcome.freed)).bold()
    );
    if outcome.trashed > 0 {
        println!(
            "  Moved ~{} to the Trash. Empty the Trash to get that space back.",
            human_bytes(outcome.trashed)
        );
    }
    if let (Some(b), Some(after)) = (before, free_space(home)) {
        println!(
            "  Free space: {} → {}",
            human_bytes(b),
            style(human_bytes(after)).green().bold()
        );
    }
    let all_time = stats::load().total_freed;
    if all_time > 0 {
        println!(
            "  {}",
            style(format!(
                "All-time freed with diskdiet: {}",
                human_bytes(all_time)
            ))
            .dim()
        );
    }
    if f.yes {
        println!("  Took {:.1}s", started.elapsed().as_secs_f64());
    }
    Ok(())
}

fn only(opts: &mut Options, projects: bool, caches: bool, docker: bool, big: bool) {
    opts.projects = projects;
    opts.caches = caches;
    opts.docker = docker;
    opts.big = big;
}

fn cmd_clean(filters: Filters, f: CleanFlags) -> Result<()> {
    let home = home()?;
    let cfg = Config::load(&home)?;
    let opts = filters.options(&cfg, &home)?;
    run_clean("clean", opts, &cfg, &home, f)
}

fn cmd_big(paths: Vec<PathBuf>, min_size: Option<u64>, f: CleanFlags) -> Result<()> {
    let home = home()?;
    let cfg = Config::load(&home)?;
    let mut opts = Options::from_config(&cfg, &home);
    only(&mut opts, false, false, false, true);
    if !paths.is_empty() {
        opts.big_paths = paths;
    }
    if let Some(m) = min_size {
        opts.big_min_size = m;
    }
    run_clean("big files", opts, &cfg, &home, f)
}

fn cmd_docker(prune: bool, f: CleanFlags) -> Result<()> {
    if !docker::available() {
        bail!("Docker is not running (or the docker CLI is not installed)");
    }
    let home = home()?;
    let cfg = Config::load(&home)?;
    if !prune {
        let mut opts = Options::from_config(&cfg, &home);
        only(&mut opts, false, false, true, false);
        opts.min_size = 0;
        return run_clean("docker", opts, &cfg, &home, f);
    }
    println!();
    println!("{}", style("Docker disk usage").bold());
    for r in docker::usage()? {
        println!(
            "  {:<15} {:>10}  reclaimable {}",
            r.kind, r.size, r.reclaimable
        );
    }
    println!();
    let mut what = vec!["stopped containers", "unused networks"];
    what.push(if f.docker_all {
        "ALL unused images, ALL build cache"
    } else {
        "dangling images, dangling build cache"
    });
    if f.docker_volumes {
        what.push("UNUSED VOLUMES");
    }
    println!("  Will remove: {}", what.join(", "));
    if !f.yes && !confirm("Continue?")? {
        println!("Cancelled.");
        return Ok(());
    }
    docker::prune(f.docker_all, f.docker_volumes)?;
    if cfg!(target_os = "macos") {
        println!();
        println!(
            "{}",
            style("Tip: Docker Desktop's disk image may take a moment to shrink. If it doesn't, use Docker Desktop → Troubleshoot → Clean / Purge data.").dim()
        );
    }
    Ok(())
}

fn cmd_alert(below: u64, no_notify: bool) -> Result<()> {
    let home = home()?;
    match alert::check(&home, below, !no_notify)? {
        alert::Check::Ok(free) => println!(
            "OK: {} free (alert below {}).",
            human_bytes(free),
            human_bytes(below)
        ),
        alert::Check::Low(free) => println!(
            "LOW: only {} free (alert below {}).",
            human_bytes(free),
            human_bytes(below)
        ),
    }
    Ok(())
}

fn cmd_stats(json: bool) -> Result<()> {
    let s = stats::load();
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!();
    println!("{}", style("diskdiet stats").bold());
    println!(
        "  Freed:   {}",
        style(human_bytes(s.total_freed)).green().bold()
    );
    if s.total_trashed > 0 {
        println!("  Trashed: {}", human_bytes(s.total_trashed));
    }
    println!("  Items:   {}", s.items_removed);
    println!("  Runs:    {}", s.runs);
    if !s.history.is_empty() {
        println!();
        println!("  {}", style("Recent runs").dim());
        let now = now_secs();
        for r in s.history.iter().rev().take(10) {
            let ago = now.saturating_sub(r.at) / 86_400;
            println!(
                "  {:>4}d ago  {:>10}  {:>4} item(s)  via {}",
                ago,
                human_bytes(r.freed + r.trashed),
                r.items,
                r.source
            );
        }
    }
    if let Some(p) = stats::path() {
        println!();
        println!("  {}", style(p.display()).dim());
    }
    Ok(())
}

fn cmd_config(action: ConfigCmd) -> Result<()> {
    let home = home()?;
    match action {
        ConfigCmd::Init => {
            let p = Config::init(&home)?;
            println!("Created {}", p.display());
        }
        ConfigCmd::Path => println!("{}", config::path(&home).display()),
        ConfigCmd::Show => {
            let p = config::path(&home);
            if p.exists() {
                Config::load(&home)?; // validate
                print!("{}", std::fs::read_to_string(&p)?);
            } else {
                println!(
                    "No config yet. Run `diskdiet config init` to create {}",
                    p.display()
                );
            }
        }
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
            trash,
            alert_below,
            alert_every,
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
            if trash {
                args.push("--trash".into());
            }
            for p in paths {
                let p = p
                    .canonicalize()
                    .with_context(|| format!("path not found: {}", p.display()))?;
                args.push(p.to_string_lossy().into_owned());
            }
            service::install(
                &home,
                &service::Job {
                    name: "clean",
                    every,
                    args,
                },
            )?;
            if let Some(below) = alert_below {
                parse_size(&below).map_err(anyhow::Error::msg)?;
                service::install(
                    &home,
                    &service::Job {
                        name: "alert",
                        every: alert_every,
                        args: vec!["alert".into(), "--below".into(), below],
                    },
                )?;
            }
            Ok(())
        }
        ServiceCmd::Uninstall => service::uninstall(&home),
        ServiceCmd::Status => service::status(&home),
    }
}

fn main() {
    diskdiet::util::augment_path();
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Scan { filters, json } => cmd_scan(filters, json),
        Cmd::Clean { filters, clean } => cmd_clean(filters, clean),
        Cmd::Big {
            paths,
            min_size,
            clean,
        } => cmd_big(paths, min_size, clean),
        Cmd::Docker { prune, clean } => cmd_docker(prune, clean),
        Cmd::Alert { below, no_notify } => cmd_alert(below, no_notify),
        Cmd::Stats { json } => cmd_stats(json),
        Cmd::Config { action } => cmd_config(action),
        Cmd::Service { action } => cmd_service(action),
    };
    if let Err(e) = res {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}
