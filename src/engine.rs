//! The shared core used by both the CLI and the desktop app: gather
//! candidates, describe them, delete them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use rayon::prelude::*;
use serde::Serialize;

use crate::bigfiles;
use crate::config::Config;
use crate::docker;
use crate::git::{GitInfo, GitInspector};
use crate::scan;
use crate::targets::CacheDef;
use crate::trashcan;
use crate::util::{dir_size, expand_tilde, has_command, pretty_path};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Project,
    Cache,
    Docker,
    Big,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Project => "Projects",
            Category::Cache => "Caches",
            Category::Docker => "Docker",
            Category::Big => "Big files",
        }
    }
}

pub enum What {
    Artifact {
        path: PathBuf,
        kind: String,
        modified: Option<SystemTime>,
        git: Option<GitInfo>,
    },
    Cache {
        def: &'static CacheDef,
        paths: Vec<PathBuf>,
    },
    Docker(docker::Obj),
    Big {
        path: PathBuf,
        kind: &'static str,
        modified: Option<SystemTime>,
    },
}

pub struct Item {
    pub what: What,
    pub size: u64,
}

/// What `clean --yes` is allowed to remove on its own.
#[derive(Clone, Copy, Default)]
pub struct AutoPolicy {
    pub docker_all: bool,
    pub docker_volumes: bool,
}

impl Item {
    pub fn category(&self) -> Category {
        match self.what {
            What::Artifact { .. } => Category::Project,
            What::Cache { .. } => Category::Cache,
            What::Docker(_) => Category::Docker,
            What::Big { .. } => Category::Big,
        }
    }

    fn is_dirty(&self) -> bool {
        matches!(&self.what, What::Artifact { git: Some(g), .. } if g.dirty)
    }

    /// Pre-selected in pickers.
    pub fn default_selected(&self) -> bool {
        match &self.what {
            What::Artifact { .. } => !self.is_dirty(),
            What::Cache { def, .. } => def.name != "Trash",
            What::Docker(o) => o.default_selected(),
            What::Big { .. } => false,
        }
    }

    /// Selected by `clean --yes` (and the background service).
    pub fn auto_selected(&self, p: AutoPolicy) -> bool {
        match &self.what {
            What::Artifact { .. } => !self.is_dirty(),
            What::Cache { def, .. } => def.name != "Trash",
            What::Docker(o) => o.auto_selected(p.docker_all, p.docker_volumes),
            What::Big { .. } => false,
        }
    }

    pub fn kind_label(&self) -> String {
        match &self.what {
            What::Artifact { kind, .. } => kind.clone(),
            What::Cache { .. } => "cache".into(),
            What::Docker(o) => o.kind_label().into(),
            What::Big { kind, .. } => (*kind).into(),
        }
    }

    pub fn age_label(&self) -> String {
        match &self.what {
            What::Artifact { modified, .. } | What::Big { modified, .. } => age(*modified),
            What::Docker(o) => o.age.clone(),
            What::Cache { .. } => "-".into(),
        }
    }

    pub fn age_days(&self) -> Option<u64> {
        match &self.what {
            What::Artifact { modified, .. } | What::Big { modified, .. } => {
                Some(modified.as_ref()?.elapsed().ok()?.as_secs() / 86_400)
            }
            _ => None,
        }
    }

    /// Main text: a path or a description.
    pub fn label(&self, home: &Path) -> String {
        match &self.what {
            What::Artifact { path, .. } | What::Big { path, .. } => pretty_path(path, home),
            What::Cache { def, paths } => format!(
                "{} · {}",
                def.name,
                paths
                    .iter()
                    .map(|p| pretty_path(p, home))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            What::Docker(o) => o.label.clone(),
        }
    }

    /// Extra warnings or hints.
    pub fn note(&self, smart_caches: bool) -> Option<String> {
        match &self.what {
            What::Artifact { git: Some(g), .. } => g.badge(),
            What::Cache { def, .. } => match def.smart {
                Some(cmd) if smart_caches && has_command(cmd[0]) => {
                    Some(format!("smart: `{}`", cmd.join(" ")))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// A path to reveal in Finder / the file manager.
    pub fn reveal_path(&self) -> Option<&Path> {
        match &self.what {
            What::Artifact { path, .. } | What::Big { path, .. } => Some(path),
            What::Cache { paths, .. } => paths.first().map(|p| p.as_path()),
            What::Docker(_) => None,
        }
    }

    pub fn to_json(&self, home: &Path, smart_caches: bool) -> JsonItem {
        JsonItem {
            category: self.category(),
            kind: self.kind_label(),
            label: self.label(home),
            path: self.reveal_path().map(|p| p.display().to_string()),
            size: self.size,
            age_days: self.age_days(),
            note: self.note(smart_caches),
            default_selected: self.default_selected(),
            git: match &self.what {
                What::Artifact { git, .. } => git.clone(),
                _ => None,
            },
        }
    }
}

#[derive(Serialize)]
pub struct JsonItem {
    pub category: Category,
    pub kind: String,
    pub label: String,
    pub path: Option<String>,
    pub size: u64,
    pub age_days: Option<u64>,
    pub note: Option<String>,
    pub default_selected: bool,
    pub git: Option<GitInfo>,
}

pub fn age(t: Option<SystemTime>) -> String {
    let Some(d) = t.and_then(|t| t.elapsed().ok()) else {
        return "?".into();
    };
    match d.as_secs() / 86_400 {
        0 => "today".into(),
        d if d < 365 => format!("{d}d"),
        d => format!("{}y", d / 365),
    }
}

/// What to look for.
#[derive(Clone)]
pub struct Options {
    pub paths: Vec<PathBuf>,
    pub older_than: Option<Duration>,
    pub min_size: u64,
    pub projects: bool,
    pub caches: bool,
    pub docker: bool,
    pub big: bool,
    pub big_paths: Vec<PathBuf>,
    pub big_min_size: u64,
    pub git: bool,
}

impl Options {
    /// Defaults merged with the user's config file.
    pub fn from_config(cfg: &Config, home: &Path) -> Self {
        let parse_size = |s: &Option<String>, d: u64| {
            s.as_deref()
                .and_then(|s| crate::util::parse_size(s).ok())
                .unwrap_or(d)
        };
        Options {
            paths: cfg.paths.iter().map(|p| expand_tilde(p, home)).collect(),
            older_than: cfg
                .older_than
                .as_deref()
                .and_then(|s| crate::util::parse_duration(s).ok()),
            min_size: parse_size(&cfg.min_size, 1024 * 1024),
            projects: true,
            caches: true,
            docker: true,
            big: false,
            big_paths: if cfg.big_paths.is_empty() {
                bigfiles::DEFAULT_PATHS
                    .iter()
                    .map(|p| home.join(p))
                    .collect()
            } else {
                cfg.big_paths
                    .iter()
                    .map(|p| expand_tilde(p, home))
                    .collect()
            },
            big_min_size: parse_size(&cfg.big_min_size, 500 * 1024 * 1024),
            git: true,
        }
    }
}

pub struct Scan {
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
}

pub fn gather(opts: &Options, cfg: &Config, home: &Path, progress: &(dyn Fn(&str) + Sync)) -> Scan {
    let mut items = Vec::new();
    let mut warnings = Vec::new();

    if opts.projects {
        let roots: Vec<PathBuf> = opts
            .paths
            .iter()
            .filter_map(|p| match p.canonicalize() {
                Ok(p) => Some(p),
                Err(_) => {
                    warnings.push(format!("path not found: {}", p.display()));
                    None
                }
            })
            .collect();
        let found = scan::find_artifacts(&roots, home, &cfg.rules, progress);
        progress(&format!("Measuring {} folders…", found.len()));
        let inspector = GitInspector::default();
        let measured: Vec<Item> = scan::measure(found)
            .into_par_iter()
            .filter(|a| {
                opts.older_than.is_none_or(|min_age| {
                    a.last_modified
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|e| e >= min_age)
                })
            })
            .map(|a| {
                let git = if opts.git {
                    a.path.parent().and_then(|p| inspector.inspect(p))
                } else {
                    None
                };
                Item {
                    size: a.size,
                    what: What::Artifact {
                        path: a.path,
                        kind: a.kind,
                        modified: a.last_modified,
                        git,
                    },
                }
            })
            .collect();
        items.extend(measured);
    }

    if opts.caches {
        progress("Measuring caches…");
        for c in scan::find_caches(home, &cfg.skip_caches) {
            items.push(Item {
                size: c.size,
                what: What::Cache {
                    def: c.def,
                    paths: c.paths,
                },
            });
        }
    }

    if opts.docker {
        progress("Asking Docker…");
        if docker::available() {
            match docker::scan() {
                Ok(objs) => items.extend(objs.into_iter().map(|o| Item {
                    size: o.size,
                    what: What::Docker(o),
                })),
                Err(e) => warnings.push(format!("{e:#}")),
            }
        }
    }

    if opts.big {
        let found = bigfiles::find(&opts.big_paths, home, opts.big_min_size, progress);
        items.extend(found.into_iter().map(|b| Item {
            size: b.size,
            what: What::Big {
                path: b.path,
                kind: b.kind,
                modified: b.modified,
            },
        }));
    }

    items.retain(|i| {
        let protected = i.reveal_path().is_some_and(|p| cfg.is_protected(p, home));
        // Stopped containers are tiny but block image removal, so always show them.
        let keep_small = matches!(&i.what, What::Docker(o) if o.kind == docker::Kind::Container)
            || i.category() == Category::Big;
        !protected && (i.size >= opts.min_size || keep_small)
    });
    items.sort_by_key(|i| std::cmp::Reverse(i.size));
    Scan { items, warnings }
}

#[derive(Clone, Copy, Default)]
pub struct DeleteOptions {
    /// Move to Trash instead of deleting permanently (not possible for Docker).
    pub trash: bool,
    /// Use tool prune commands for caches that support it.
    pub smart_caches: bool,
}

#[derive(Default, Debug)]
pub struct Outcome {
    /// Bytes actually released.
    pub freed: u64,
    /// Bytes moved to the Trash (released once the Trash is emptied).
    pub trashed: u64,
    pub removed: usize,
    pub errors: Vec<String>,
}

fn run_tool(cmd: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd[0])
        .args(&cmd[1..])
        .output()
        .map_err(|e| format!("{}: {e}", cmd[0]))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Delete `items`. `progress(done, total)` is called after each item.
pub fn delete(
    items: &[&Item],
    home: &Path,
    opts: DeleteOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Outcome {
    let total = items.len();
    let done = AtomicUsize::new(0);
    let freed = AtomicU64::new(0);
    let trashed = AtomicU64::new(0);
    let tick = || progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);

    let (fs_items, mut docker_items): (Vec<&Item>, Vec<&Item>) = items
        .iter()
        .copied()
        .partition(|i| !matches!(i.what, What::Docker(_)));

    // Trashing goes through the OS one call at a time, so keep it sequential.
    let mut errors: Vec<String> = if opts.trash {
        fs_items
            .iter()
            .filter_map(|item| {
                let res = delete_fs(item, home, opts, &freed, &trashed);
                tick();
                res.err()
            })
            .collect()
    } else {
        fs_items
            .par_iter()
            .filter_map(|item| {
                let res = delete_fs(item, home, opts, &freed, &trashed);
                tick();
                res.err()
            })
            .collect()
    };

    // Docker: containers first, then images, volumes, build cache.
    docker_items.sort_by_key(|i| match &i.what {
        What::Docker(o) => o.kind,
        _ => docker::Kind::BuildCache,
    });
    for item in docker_items {
        if let What::Docker(o) = &item.what {
            match o.delete() {
                Ok(()) => {
                    freed.fetch_add(item.size, Ordering::Relaxed);
                }
                Err(e) => errors.push(format!("{}: {e:#}", o.label)),
            }
        }
        tick();
    }

    Outcome {
        freed: freed.into_inner(),
        trashed: trashed.into_inner(),
        removed: total - errors.len(),
        errors,
    }
}

fn delete_fs(
    item: &Item,
    home: &Path,
    opts: DeleteOptions,
    freed: &AtomicU64,
    trashed: &AtomicU64,
) -> Result<(), String> {
    let pp = |p: &Path| pretty_path(p, home);
    match &item.what {
        What::Artifact { path, .. } | What::Big { path, .. } => {
            if opts.trash {
                trashcan::move_to_trash(std::slice::from_ref(path))
                    .map_err(|e| format!("{}: {e:#}", pp(path)))?;
                trashed.fetch_add(item.size, Ordering::Relaxed);
            } else {
                let res = if path.is_dir() {
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                };
                res.map_err(|e| format!("{}: {e}", pp(path)))?;
                freed.fetch_add(item.size, Ordering::Relaxed);
            }
        }
        What::Cache { def, paths } => {
            if let Some(cmd) = def.smart.filter(|c| opts.smart_caches && has_command(c[0])) {
                run_tool(cmd).map_err(|e| format!("{}: {e}", def.name))?;
                let after: u64 = paths.iter().map(|p| dir_size(p)).sum();
                freed.fetch_add(item.size.saturating_sub(after), Ordering::Relaxed);
                return Ok(());
            }
            for p in paths {
                if opts.trash && def.name != "Trash" {
                    trashcan::trash_contents(p)
                        .map_err(|e| format!("{} ({}): {e:#}", def.name, pp(p)))?;
                } else {
                    scan::clear_dir_contents(p)
                        .map_err(|e| format!("{} ({}): {e}", def.name, pp(p)))?;
                }
            }
            if opts.trash && def.name != "Trash" {
                trashed.fetch_add(item.size, Ordering::Relaxed);
            } else {
                freed.fetch_add(item.size, Ordering::Relaxed);
            }
        }
        What::Docker(_) => {}
    }
    Ok(())
}

/// Open the file manager with `path` selected.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
}
