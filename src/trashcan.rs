//! Move things to the system Trash instead of deleting them permanently.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub fn move_to_trash(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        // Faster than asking Finder and doesn't play the trash sound per item.
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete_all(paths).map_err(|e| anyhow!("{e}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete_all(paths).map_err(|e| anyhow!("{e}"))
    }
}

/// Trash every entry inside `dir`, keeping `dir` itself.
pub fn trash_contents(dir: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .collect();
    move_to_trash(&entries)
}
