//! Optional user config at `~/.config/diskdiet/config.toml`
//! (or `$XDG_CONFIG_HOME/diskdiet/config.toml`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::util::expand_tilde;

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Folders to scan when none are given on the command line.
    pub paths: Vec<String>,
    /// Never touch anything inside these folders.
    pub protected: Vec<String>,
    /// Skip any path that has one of these folder names in it.
    pub exclude: Vec<String>,
    /// Default for `--older-than` (e.g. "14d").
    pub older_than: Option<String>,
    /// Default for `--min-size` (e.g. "10MB").
    pub min_size: Option<String>,
    /// Move to Trash instead of deleting permanently.
    pub trash: bool,
    /// "numbers" (default) or "tui".
    pub picker: Option<String>,
    /// Use tool commands (pnpm store prune, brew cleanup, ...) where possible. Default true.
    pub smart_caches: Option<bool>,
    /// Cache names to ignore, e.g. ["Maven repository", "Trash"].
    pub skip_caches: Vec<String>,
    /// Minimum size for "big files" mode (e.g. "500MB").
    pub big_min_size: Option<String>,
    /// Folders searched in "big files" mode.
    pub big_paths: Vec<String>,
    /// Extra artifact rules.
    pub rules: Vec<CustomRule>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CustomRule {
    /// Folder name to match, e.g. "dist".
    pub dir: String,
    /// The parent folder must contain one of these files. Empty = always match.
    #[serde(default)]
    pub markers: Vec<String>,
    /// Label shown in the "type" column.
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "custom".into()
}

pub const TEMPLATE: &str = r#"# diskdiet config (beta)
# Every key is optional. Delete what you don't need.

# Folders to scan when you run `diskdiet clean` without paths.
# paths = ["~/code", "~/projects"]

# Never touch anything inside these folders.
# protected = ["~/code/client-work"]

# Skip any path that contains one of these folder names.
# exclude = ["keep-me"]

# Defaults for command-line flags.
# older_than = "14d"
# min_size = "10MB"

# Move things to the Trash instead of deleting them permanently.
# (Space is only freed once you empty the Trash.)
# trash = false

# Picker style: "numbers" or "tui" (full screen, arrow keys).
# picker = "numbers"

# Use `pnpm store prune`, `brew cleanup` and `uv cache prune` instead of
# wiping those caches.
# smart_caches = true

# Caches to ignore completely.
# skip_caches = ["Maven repository"]

# Big files mode.
# big_min_size = "500MB"
# big_paths = ["~/Downloads", "~/Desktop", "~/Documents", "~/Movies"]

# Extra folders to treat as junk.
# [[rules]]
# dir = "dist"
# markers = ["vite.config.ts", "vite.config.js"]
# kind = "Vite"
"#;

pub fn path(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("diskdiet/config.toml")
}

impl Config {
    pub fn load(home: &Path) -> Result<Self> {
        let p = path(home);
        if !p.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config {}", p.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn init(home: &Path) -> Result<PathBuf> {
        let p = path(home);
        if p.exists() {
            anyhow::bail!("{} already exists", p.display());
        }
        fs::create_dir_all(p.parent().unwrap())?;
        fs::write(&p, TEMPLATE)?;
        Ok(p)
    }
}

/// Write the desktop app's settings into the config file, keeping every
/// other key. (Comments in the file are not preserved.)
pub fn save_defaults(home: &Path, values: &[(&str, toml::Value)]) -> Result<PathBuf> {
    let p = path(home);
    let mut table: toml::Table = if p.exists() {
        toml::from_str(&fs::read_to_string(&p)?)
            .with_context(|| format!("invalid config {}", p.display()))?
    } else {
        toml::Table::new()
    };
    for (k, v) in values {
        table.insert((*k).to_string(), v.clone());
    }
    // Make sure the result still loads.
    let text = toml::to_string_pretty(&table)?;
    Config::parse(&text)?;
    fs::create_dir_all(p.parent().unwrap())?;
    fs::write(
        &p,
        format!("# diskdiet config (written by the desktop app)\n{text}"),
    )?;
    Ok(p)
}

impl Config {
    pub fn protected_paths(&self, home: &Path) -> Vec<PathBuf> {
        self.protected
            .iter()
            .map(|p| expand_tilde(p, home))
            .collect()
    }

    /// True when `path` must not be touched.
    pub fn is_protected(&self, path: &Path, home: &Path) -> bool {
        self.protected_paths(home)
            .iter()
            .any(|p| path.starts_with(p))
            || path.components().any(|c| {
                let c = c.as_os_str().to_string_lossy();
                self.exclude.iter().any(|e| *e == c)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses() {
        let c = Config::parse(TEMPLATE).unwrap();
        assert!(c.paths.is_empty());
    }

    #[test]
    fn full_config() {
        let c = Config::parse(
            r#"
            paths = ["~/code"]
            protected = ["~/code/keep"]
            exclude = ["vendor-lock"]
            trash = true
            [[rules]]
            dir = "dist"
            markers = ["vite.config.ts"]
            "#,
        )
        .unwrap();
        let home = Path::new("/home/u");
        assert!(c.trash);
        assert_eq!(c.rules[0].kind, "custom");
        assert!(c.is_protected(Path::new("/home/u/code/keep/node_modules"), home));
        assert!(c.is_protected(Path::new("/x/vendor-lock/node_modules"), home));
        assert!(!c.is_protected(Path::new("/home/u/code/app/node_modules"), home));
    }

    #[test]
    fn save_defaults_roundtrip() {
        let home = std::env::temp_dir().join(format!("diskdiet-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        std::env::remove_var("XDG_CONFIG_HOME");
        save_defaults(&home, &[("trash", toml::Value::Boolean(true))]).unwrap();
        save_defaults(
            &home,
            &[("paths", toml::Value::Array(vec!["~/code".into()]))],
        )
        .unwrap();
        let c = Config::load(&home).unwrap();
        assert!(c.trash);
        assert_eq!(c.paths, vec!["~/code"]);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn rejects_typos() {
        assert!(Config::parse("trsh = true").is_err());
    }
}
