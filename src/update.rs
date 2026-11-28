//! "Is there a newer release?" for the desktop app.
//!
//! Reads the public GitHub releases feed with `curl`, at most once a day, and
//! never downloads or installs anything. Turn it off with
//! `update_check = false` in the config.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::util::now_secs;

pub const FEED: &str = "https://github.com/anggerari/diskdiet/releases.atom";
pub const RELEASES: &str = "https://github.com/anggerari/diskdiet/releases";
const DAY: u64 = 86_400;

#[derive(Debug, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    /// e.g. ("beta", 4). None for a final release.
    pre: Option<(String, u64)>,
}

fn parse(v: &str) -> Option<Version> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let mut nums = core.split('.').map(|n| n.parse::<u64>().ok());
    let core = [nums.next()??, nums.next()??, nums.next()??];
    let pre = match pre {
        None => None,
        Some(p) => {
            let (name, n) = p.split_once('.').unwrap_or((p, "0"));
            Some((name.to_string(), n.parse().ok()?))
        }
    };
    Some(Version { core, pre })
}

fn cmp(a: &Version, b: &Version) -> Ordering {
    a.core.cmp(&b.core).then_with(|| match (&a.pre, &b.pre) {
        (None, None) => Ordering::Equal,
        // 1.0.0 is newer than 1.0.0-beta.9
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    })
}

/// True when `latest` is a newer version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => cmp(&l, &c) == Ordering::Greater,
        _ => false,
    }
}

/// Newest release tag in a GitHub releases Atom feed (newest entry first).
pub fn latest_from_feed(xml: &str) -> Option<String> {
    let start = xml.find("/releases/tag/")? + "/releases/tag/".len();
    let rest = &xml[start..];
    let end = rest.find(['"', '<', '\''])?;
    let tag = rest[..end].trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    checked_at: u64,
    latest: Option<String>,
}

fn cache_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("diskdiet/update.json"))
}

fn fetch() -> Option<String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "8", FEED])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| latest_from_feed(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

fn check_with(path: &Path, now: u64, fetch: impl FnOnce() -> Option<String>) -> Option<String> {
    let cached: Cache = fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if now.saturating_sub(cached.checked_at) < DAY {
        return cached.latest;
    }
    let latest = fetch();
    // Remember failures too, so an offline laptop doesn't retry every launch.
    let c = Cache {
        checked_at: now,
        latest: latest.clone().or(cached.latest),
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, serde_json::to_vec(&c).unwrap_or_default());
    c.latest
}

/// The newest release tag if it's newer than this build. Blocks on the
/// network at most once a day; call it from a background thread.
pub fn newer_release() -> Option<String> {
    let latest = check_with(&cache_path()?, now_secs(), fetch)?;
    is_newer(&latest, crate::VERSION).then_some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(is_newer("v0.1.0-beta.5", "0.1.0-beta.4"));
        assert!(is_newer("v0.1.0-beta.10", "0.1.0-beta.9"));
        assert!(is_newer("v0.1.0", "0.1.0-beta.9"));
        assert!(is_newer("v0.2.0-beta.1", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.0-beta.4", "0.1.0-beta.4"));
        assert!(!is_newer("v0.1.0-beta.3", "0.1.0-beta.4"));
        assert!(!is_newer("v0.1.0-beta.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn reads_the_feed() {
        let xml = r#"<feed><entry><id>tag:github.com,2008:Repository/1/v0.1.0-beta.5</id>
            <link rel="alternate" type="text/html" href="https://github.com/anggerari/diskdiet/releases/tag/v0.1.0-beta.5"/>
            </entry><entry><link href="https://github.com/anggerari/diskdiet/releases/tag/v0.1.0-beta.4"/></entry></feed>"#;
        assert_eq!(latest_from_feed(xml).as_deref(), Some("v0.1.0-beta.5"));
        assert_eq!(latest_from_feed("<feed></feed>"), None);
    }

    #[test]
    fn checks_at_most_once_a_day() {
        let dir = std::env::temp_dir().join(format!("diskdiet-update-{}", std::process::id()));
        let p = dir.join("update.json");
        let _ = fs::remove_dir_all(&dir);
        let got = check_with(&p, 1000, || Some("v9.0.0".into()));
        assert_eq!(got.as_deref(), Some("v9.0.0"));
        // Within a day: cached, no fetch.
        let got = check_with(&p, 1000 + DAY - 1, || panic!("fetched again"));
        assert_eq!(got.as_deref(), Some("v9.0.0"));
        // A day later and offline: keep the last known answer.
        let got = check_with(&p, 1000 + DAY, || None);
        assert_eq!(got.as_deref(), Some("v9.0.0"));
        let _ = fs::remove_dir_all(dir);
    }
}
