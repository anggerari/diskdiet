use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use indicatif::ProgressBar;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::targets::{is_artifact_name, ArtifactRule, CACHES, RULES};
use crate::util::dir_size;

pub struct Artifact {
    pub path: PathBuf,
    pub kind: &'static str,
    pub size: u64,
    /// Most recent modification time of the owning project (excluding artifacts).
    pub last_modified: Option<SystemTime>,
}

pub struct Cache {
    pub name: &'static str,
    pub paths: Vec<PathBuf>,
    pub size: u64,
}

fn match_rule(name: &str, parent: &Path) -> Option<&'static ArtifactRule> {
    RULES
        .iter()
        .find(|r| r.dir == name && r.marker.matches(parent))
}

fn should_skip(name: &str, path: &Path, home: &Path) -> bool {
    // Hidden folders (.git, .config, .cache, ...) are never projects; global
    // caches are handled separately. ~/Library is macOS app data.
    name.starts_with('.') || (name == "Library" && path.parent() == Some(home))
}

/// Walk `roots` and return every artifact folder found (without measuring).
pub fn find_artifacts(
    roots: &[PathBuf],
    home: &Path,
    pb: &ProgressBar,
) -> Vec<(PathBuf, &'static str)> {
    let mut found = Vec::new();
    let mut visited: u64 = 0;
    for root in roots {
        let mut it = WalkDir::new(root).follow_links(false).into_iter();
        while let Some(entry) = it.next() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_dir() || entry.depth() == 0 {
                continue;
            }
            visited += 1;
            if visited.is_multiple_of(500) {
                pb.set_message(format!(
                    "Scanning… {visited} folders, {} found",
                    found.len()
                ));
            }
            let name = entry.file_name().to_string_lossy();
            let path = entry.path();
            if let Some(parent) = path.parent() {
                if let Some(rule) = match_rule(&name, parent) {
                    found.push((path.to_path_buf(), rule.kind));
                    it.skip_current_dir();
                    continue;
                }
            }
            if should_skip(&name, path, home) {
                it.skip_current_dir();
            }
        }
    }
    found.sort();
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

fn project_mtime(project: &Path) -> Option<SystemTime> {
    fs::read_dir(project)
        .ok()?
        .flatten()
        .filter(|e| !is_artifact_name(&e.file_name().to_string_lossy()))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// Measure sizes in parallel.
pub fn measure(found: Vec<(PathBuf, &'static str)>) -> Vec<Artifact> {
    found
        .into_par_iter()
        .map(|(path, kind)| Artifact {
            size: dir_size(&path),
            last_modified: path.parent().and_then(project_mtime),
            path,
            kind,
        })
        .collect()
}

pub fn find_caches(home: &Path) -> Vec<Cache> {
    CACHES
        .par_iter()
        .filter_map(|def| {
            let paths: Vec<PathBuf> = def
                .paths
                .iter()
                .map(|p| home.join(p))
                .filter(|p| p.is_dir())
                .collect();
            if paths.is_empty() {
                return None;
            }
            let size = paths.iter().map(|p| dir_size(p)).sum();
            Some(Cache {
                name: def.name,
                paths,
                size,
            })
        })
        .collect()
}

/// Delete everything inside `dir`, keeping `dir` itself (tools expect cache roots to exist).
pub fn clear_dir_contents(dir: &Path) -> io::Result<()> {
    let mut first_err = None;
    for entry in fs::read_dir(dir)? {
        let res = entry.and_then(|e| {
            if e.file_type()?.is_dir() {
                fs::remove_dir_all(e.path())
            } else {
                fs::remove_file(e.path())
            }
        });
        if let Err(e) = res {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("diskdiet-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_only_real_artifacts() {
        let root = tmp("scan");
        // Real node project
        fs::create_dir_all(root.join("web/node_modules/left-pad")).unwrap();
        fs::write(root.join("web/package.json"), "{}").unwrap();
        // node_modules nested inside node_modules must not be reported twice
        fs::create_dir_all(root.join("web/node_modules/a/node_modules")).unwrap();
        // Rust project
        fs::create_dir_all(root.join("crab/target/debug")).unwrap();
        fs::write(root.join("crab/Cargo.toml"), "").unwrap();
        // A hand-written `build` folder with no build system marker: keep it
        fs::create_dir_all(root.join("docs/build")).unwrap();
        // A `target` folder without Cargo.toml: keep it
        fs::create_dir_all(root.join("misc/target")).unwrap();

        let found = find_artifacts(
            std::slice::from_ref(&root),
            Path::new("/nonexistent"),
            &ProgressBar::hidden(),
        );
        let mut names: Vec<_> = found
            .iter()
            .map(|(p, k)| (p.strip_prefix(&root).unwrap().to_path_buf(), *k))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                (PathBuf::from("crab/target"), "Rust"),
                (PathBuf::from("web/node_modules"), "Node"),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clears_contents_but_keeps_dir() {
        let root = tmp("clear");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("f.txt"), "x").unwrap();
        clear_dir_contents(&root).unwrap();
        assert!(root.is_dir());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
