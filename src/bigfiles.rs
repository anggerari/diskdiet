//! "Big files" mode: large forgotten files (installers, archives, videos, VM
//! images) plus iOS backups and Xcode archives.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::targets::is_artifact_name;
use crate::util::dir_size;

pub struct BigFile {
    pub path: PathBuf,
    pub kind: &'static str,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub const DEFAULT_PATHS: &[&str] = &["Downloads", "Desktop", "Documents", "Movies"];

pub fn kind_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "dmg" | "pkg" | "iso" | "img" | "msi" | "exe" | "appimage" | "deb" | "rpm" => "installer",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => "archive",
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" => "video",
        "vmdk" | "vdi" | "qcow2" | "vhd" | "vhdx" | "raw" | "ova" => "VM disk",
        "ipa" | "apk" | "aab" | "xcarchive" => "app build",
        "log" => "log",
        _ => "file",
    }
}

#[cfg(unix)]
fn on_disk(m: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.blocks() * 512
}
#[cfg(not(unix))]
fn on_disk(m: &fs::Metadata) -> u64 {
    m.len()
}

/// Bundles that look like folders but must be treated as one thing.
fn is_bundle(name: &str) -> bool {
    [
        ".app",
        ".photoslibrary",
        ".musiclibrary",
        ".fcpbundle",
        ".tvlibrary",
        ".xcarchive",
    ]
    .iter()
    .any(|ext| name.ends_with(ext))
}

pub fn find(
    roots: &[PathBuf],
    home: &Path,
    min_size: u64,
    progress: &(dyn Fn(&str) + Sync),
) -> Vec<BigFile> {
    let mut out = Vec::new();
    for root in roots.iter().filter(|r| r.is_dir()) {
        progress(&format!(
            "Looking for big files in {}…",
            crate::util::pretty_path(root, home)
        ));
        let mut it = WalkDir::new(root).follow_links(false).into_iter();
        while let Some(Ok(entry)) = it.next() {
            let name = entry.file_name().to_string_lossy();
            if entry.depth() > 0 && entry.file_type().is_dir() {
                if name.starts_with('.') || is_artifact_name(&name) || is_bundle(&name) {
                    it.skip_current_dir();
                }
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(m) = entry.metadata() else { continue };
            let size = on_disk(&m);
            if size >= min_size {
                out.push(BigFile {
                    kind: kind_for(entry.path()),
                    path: entry.into_path(),
                    size,
                    modified: m.modified().ok(),
                });
            }
        }
    }

    // iOS / iPadOS device backups (needs Full Disk Access on macOS).
    let backups = home.join("Library/Application Support/MobileSync/Backup");
    if let Ok(rd) = fs::read_dir(&backups) {
        progress("Measuring iOS backups…");
        for e in rd.flatten().filter(|e| e.path().is_dir()) {
            let size = dir_size(&e.path());
            if size >= min_size {
                out.push(BigFile {
                    modified: e.metadata().ok().and_then(|m| m.modified().ok()),
                    path: e.path(),
                    kind: "iOS backup",
                    size,
                });
            }
        }
    }

    // Xcode archives, one per app build.
    let archives = home.join("Library/Developer/Xcode/Archives");
    if archives.is_dir() {
        progress("Measuring Xcode archives…");
        for e in WalkDir::new(&archives).max_depth(2).into_iter().flatten() {
            if e.file_type().is_dir() && e.file_name().to_string_lossy().ends_with(".xcarchive") {
                let size = dir_size(e.path());
                if size >= min_size {
                    out.push(BigFile {
                        modified: e.metadata().ok().and_then(|m| m.modified().ok()),
                        path: e.into_path(),
                        kind: "Xcode archive",
                        size,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_for(Path::new("a/Docker.dmg")), "installer");
        assert_eq!(kind_for(Path::new("a/b.TAR")), "archive");
        assert_eq!(kind_for(Path::new("a/b.mov")), "video");
        assert_eq!(kind_for(Path::new("a/b")), "file");
    }

    #[test]
    fn finds_big_files_and_skips_node_modules() {
        let root = std::env::temp_dir().join(format!("diskdiet-big-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("big.zip"), vec![1u8; 300_000]).unwrap();
        fs::write(root.join("small.txt"), b"x").unwrap();
        fs::write(root.join("node_modules/huge.bin"), vec![1u8; 300_000]).unwrap();
        let found = find(
            std::slice::from_ref(&root),
            Path::new("/nonexistent"),
            200_000,
            &|_| {},
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "archive");
        fs::remove_dir_all(root).unwrap();
    }
}
