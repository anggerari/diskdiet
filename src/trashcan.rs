//! Move things to the system Trash instead of deleting them permanently,
//! remembering where each one landed so it can be put back later.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

/// One thing that went to the Trash.
#[derive(Debug, Clone)]
pub struct Trashed {
    pub original: PathBuf,
    /// Where to find it again: the file inside the Trash on macOS, the
    /// trash-info id on Linux. `None` if the OS didn't tell us.
    pub handle: Option<String>,
}

/// Trash `paths`, pushing each one that made it onto `out` (also when a
/// later one fails, so a partial clean can still be undone).
pub fn move_to_trash(paths: &[PathBuf], out: &mut Vec<Trashed>) -> Result<()> {
    for p in paths {
        out.push(trash_one(p)?);
    }
    Ok(())
}

/// Trash every entry inside `dir`, keeping `dir` itself.
pub fn trash_contents(dir: &Path, out: &mut Vec<Trashed>) -> Result<()> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .collect();
    move_to_trash(&entries, out)
}

#[cfg(target_os = "macos")]
fn trash_one(path: &Path) -> Result<Trashed> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let Some(s) = path.to_str() else {
        // Not valid UTF-8: let the trash crate deal with it (no undo).
        trash::delete(path).map_err(|e| anyhow!("{e}"))?;
        return Ok(Trashed {
            original: path.to_path_buf(),
            handle: None,
        });
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(s));
    let mut out = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut out))
        .map_err(|e| anyhow!("{}", e.localizedDescription()))?;
    Ok(Trashed {
        original: path.to_path_buf(),
        handle: out.and_then(|u| u.path()).map(|p| p.to_string()),
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn trash_one(path: &Path) -> Result<Trashed> {
    let original = std::path::absolute(path)?;
    trash::delete(&original).map_err(|e| anyhow!("{e}"))?;
    // The freedesktop trash doesn't hand back an id; find the newest entry
    // that came from this path.
    let handle = trash::os_limited::list().ok().and_then(|items| {
        items
            .into_iter()
            .filter(|i| i.original_path() == original)
            .max_by_key(|i| i.time_deleted)
            .map(|i| i.id.to_string_lossy().into_owned())
    });
    Ok(Trashed { original, handle })
}

#[cfg(not(unix))]
fn trash_one(path: &Path) -> Result<Trashed> {
    trash::delete(path).map_err(|e| anyhow!("{e}"))?;
    Ok(Trashed {
        original: path.to_path_buf(),
        handle: None,
    })
}

/// Put one trashed thing back where it came from.
pub fn restore(original: &Path, handle: &str) -> Result<()> {
    if original.symlink_metadata().is_ok() {
        return Err(anyhow!("something new is already at that path"));
    }
    restore_impl(original, handle)
}

#[cfg(target_os = "macos")]
fn restore_impl(original: &Path, handle: &str) -> Result<()> {
    let from = Path::new(handle);
    if from.symlink_metadata().is_err() {
        return Err(anyhow!("it's no longer in the Trash"));
    }
    if let Some(parent) = original.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, original)?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn restore_impl(original: &Path, handle: &str) -> Result<()> {
    let item = trash::os_limited::list()
        .map_err(|e| anyhow!("{e}"))?
        .into_iter()
        .find(|i| i.id.to_string_lossy() == handle)
        .ok_or_else(|| anyhow!("it's no longer in the Trash"))?;
    if let Some(parent) = original.parent() {
        std::fs::create_dir_all(parent)?;
    }
    trash::os_limited::restore_all([item]).map_err(|e| anyhow!("{e}"))
}

#[cfg(not(unix))]
fn restore_impl(_original: &Path, _handle: &str) -> Result<()> {
    Err(anyhow!("restoring isn't supported on this platform"))
}
