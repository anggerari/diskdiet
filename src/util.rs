use std::path::Path;
use std::process::Command;
use std::time::Duration;

use walkdir::WalkDir;

/// Format a byte count as a human readable string (binary units).
pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Parse durations like `30d`, `2w`, `12h`, `45m`, `90s`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("invalid duration '{s}' (examples: 30d, 2w, 12h)"))?;
    let mult = match unit.trim().to_ascii_lowercase().as_str() {
        "s" => 1,
        "m" | "min" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => {
            return Err(format!(
                "invalid duration unit in '{s}' (use s, m, h, d or w)"
            ))
        }
    };
    Ok(Duration::from_secs(n * mult))
}

/// Parse sizes like `500KB`, `100MB`, `1.5G`, `0`.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num
        .parse()
        .map_err(|_| format!("invalid size '{s}' (examples: 100MB, 1GB)"))?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("invalid size unit in '{s}'")),
    };
    Ok((n * mult) as u64)
}

/// Actual on-disk usage of a directory tree (hard links counted once).
#[cfg(unix)]
pub fn dir_size(path: &Path) -> u64 {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;

    let mut seen = HashSet::new();
    let mut total = 0;
    for entry in WalkDir::new(path).follow_links(false).into_iter().flatten() {
        let Ok(m) = entry.metadata() else { continue };
        if !m.is_dir() && m.nlink() > 1 && !seen.insert((m.dev(), m.ino())) {
            continue;
        }
        total += m.blocks() * 512;
    }
    total
}

#[cfg(not(unix))]
pub fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// `(total, free)` bytes of the filesystem containing `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let out = Command::new("df").arg("-Pk").arg(path).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut cols = s.lines().nth(1)?.split_whitespace().skip(1);
    let total: u64 = cols.next()?.parse().ok()?;
    let free: u64 = cols.nth(1)?.parse().ok()?;
    Some((total * 1024, free * 1024))
}

/// Free space available on the filesystem containing `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    disk_space(path).map(|(_, free)| free)
}

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(p: &str, home: &Path) -> std::path::PathBuf {
    if p == "~" {
        home.to_path_buf()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home.join(rest)
    } else {
        std::path::PathBuf::from(p)
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// GUI apps and background services start with a minimal PATH. Add the usual
/// places where docker, git, brew, pnpm and friends live.
pub fn augment_path() {
    let home = dirs::home_dir().unwrap_or_default();
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    let extra = [
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/usr/local/bin"),
        std::path::PathBuf::from("/Applications/Docker.app/Contents/Resources/bin"),
        home.join(".cargo/bin"),
        home.join(".local/bin"),
        home.join("Library/pnpm"),
        home.join(".local/share/pnpm"),
        home.join(".bun/bin"),
        std::path::PathBuf::from("/usr/bin"),
        std::path::PathBuf::from("/bin"),
    ];
    for p in extra {
        if p.is_dir() && !paths.contains(&p) {
            paths.push(p);
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

/// True when `cmd` can be found on PATH.
pub fn has_command(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

/// Replace the home directory prefix with `~` for display.
pub fn pretty_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(
            parse_duration("30d").unwrap(),
            Duration::from_secs(30 * 86_400)
        );
        assert_eq!(
            parse_duration("2w").unwrap(),
            Duration::from_secs(2 * 604_800)
        );
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("10y").is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("1KB").unwrap(), 1024);
        assert_eq!(parse_size("1.5g").unwrap(), 1_610_612_736);
        assert!(parse_size("1XB").is_err());
    }

    #[test]
    fn human() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }
}
