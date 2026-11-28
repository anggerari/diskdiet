//! Undo for cleans that used the Trash.
//!
//! Every clean that moves things to the Trash saves a small record of where
//! each item came from and where it landed. `restore` puts them back.
//! Permanent deletes and Docker removals can't be undone, so they aren't kept.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::trashcan::{self, Trashed};
use crate::util::now_secs;

/// Cleans to remember. Older ones are dropped.
const KEEP: usize = 20;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    /// Where the item lived before it was trashed.
    pub original: PathBuf,
    /// Where to find it in the Trash (see `trashcan::Trashed::handle`).
    pub handle: String,
    /// Which scan item it belonged to, e.g. "node_modules" or "npm cache".
    pub label: String,
    /// Size of the whole scan item (shared by entries of the same item).
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub restored: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Clean {
    /// Unix seconds; also the id used by `diskdiet undo --id`.
    pub at: u64,
    /// "cli", "auto" or "gui".
    pub source: String,
    pub entries: Vec<Entry>,
}

impl Clean {
    pub fn pending(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.restored)
    }

    pub fn can_restore(&self) -> bool {
        self.pending().next().is_some()
    }

    /// Total size of the scan items that can still be restored.
    pub fn pending_size(&self) -> u64 {
        let mut seen = std::collections::HashSet::new();
        self.pending()
            .filter(|e| seen.insert(&e.label))
            .map(|e| e.size)
            .sum()
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct Log {
    pub cleans: Vec<Clean>,
}

pub fn path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("diskdiet/undo.json"))
}

pub fn load() -> Log {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

fn load_from(p: &Path) -> Log {
    fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_to(p: &Path, log: &Log) -> Result<()> {
    fs::create_dir_all(p.parent().unwrap_or(Path::new(".")))?;
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(log)?)?;
    fs::rename(tmp, p)?;
    Ok(())
}

/// One scan item's trashed paths.
#[derive(Debug)]
pub struct Batch {
    pub label: String,
    pub size: u64,
    pub trashed: Vec<Trashed>,
}

/// Remember a clean so it can be undone. Items without a handle are skipped.
pub fn record(batches: Vec<Batch>, source: &str) -> Result<()> {
    match path() {
        Some(p) => record_to(&p, batches, source, now_secs()),
        None => Ok(()),
    }
}

fn record_to(p: &Path, batches: Vec<Batch>, source: &str, at: u64) -> Result<()> {
    let entries: Vec<Entry> = batches
        .into_iter()
        .flat_map(|b| {
            let (label, size) = (b.label, b.size);
            b.trashed.into_iter().filter_map(move |t| {
                Some(Entry {
                    original: t.original,
                    handle: t.handle?,
                    label: label.clone(),
                    size,
                    restored: false,
                })
            })
        })
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    let mut log = load_from(p);
    // Two cleans in the same second would share an id: bump it.
    let at = log.cleans.last().map_or(at, |c| at.max(c.at + 1));
    log.cleans.push(Clean {
        at,
        source: source.into(),
        entries,
    });
    if log.cleans.len() > KEEP {
        let extra = log.cleans.len() - KEEP;
        log.cleans.drain(..extra);
    }
    save_to(p, &log)
}

/// The newest clean that still has something to put back.
pub fn last_restorable() -> Option<Clean> {
    load().cleans.into_iter().rev().find(Clean::can_restore)
}

#[derive(Debug, Default)]
pub struct Restored {
    pub restored: usize,
    /// Size of the scan items that came back completely.
    pub bytes: u64,
    /// "<path>: <why>" for everything that couldn't be put back.
    pub skipped: Vec<String>,
}

/// Put back everything from the clean with id `at` that's still in the Trash.
/// `progress(done, total)` is called after each entry.
pub fn restore(at: u64, progress: &dyn Fn(usize, usize)) -> Result<Restored> {
    match path() {
        Some(p) => restore_from(&p, at, progress, &|o, h| trashcan::restore(o, h)),
        None => Ok(Restored::default()),
    }
}

type RestoreFn<'a> = dyn Fn(&Path, &str) -> Result<()> + 'a;

fn restore_from(
    p: &Path,
    at: u64,
    progress: &dyn Fn(usize, usize),
    put_back: &RestoreFn,
) -> Result<Restored> {
    let mut log = load_from(p);
    let Some(clean) = log.cleans.iter_mut().find(|c| c.at == at) else {
        anyhow::bail!("no clean with id {at} (see `diskdiet undo --list`)");
    };
    let total = clean.pending().count();
    let mut out = Restored::default();
    let mut failed_labels = std::collections::HashSet::new();
    let mut touched = std::collections::HashSet::new();
    let mut done = 0;
    for e in clean.entries.iter_mut().filter(|e| !e.restored) {
        touched.insert(e.label.clone());
        match put_back(&e.original, &e.handle) {
            Ok(()) => {
                e.restored = true;
                out.restored += 1;
            }
            Err(err) => {
                // Gone from the Trash for good: stop offering it.
                if err.to_string().contains("no longer in the Trash") {
                    e.restored = true;
                }
                failed_labels.insert(e.label.clone());
                out.skipped
                    .push(format!("{}: {err:#}", e.original.display()));
            }
        }
        done += 1;
        progress(done, total);
    }
    let mut seen = std::collections::HashSet::new();
    out.bytes = clean
        .entries
        .iter()
        .filter(|e| touched.contains(&e.label) && !failed_labels.contains(&e.label))
        .filter(|e| seen.insert(e.label.clone()))
        .map(|e| e.size)
        .sum();
    save_to(p, &log)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("diskdiet-undo-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn batch(label: &str, size: u64, items: &[(&str, Option<&str>)]) -> Batch {
        Batch {
            label: label.into(),
            size,
            trashed: items
                .iter()
                .map(|(o, h)| Trashed {
                    original: PathBuf::from(o),
                    handle: h.map(String::from),
                })
                .collect(),
        }
    }

    #[test]
    fn records_only_restorable_entries_and_caps_history() {
        let d = tmp("record");
        let p = d.join("undo.json");
        record_to(&p, vec![batch("a", 1, &[("/x", None)])], "cli", 10).unwrap();
        assert!(
            load_from(&p).cleans.is_empty(),
            "nothing restorable, nothing saved"
        );

        for i in 0..(KEEP as u64 + 5) {
            record_to(
                &p,
                vec![batch("a", 1, &[("/x", Some("/t"))])],
                "cli",
                100 + i,
            )
            .unwrap();
        }
        let log = load_from(&p);
        assert_eq!(log.cleans.len(), KEEP);
        assert_eq!(log.cleans.last().unwrap().at, 100 + KEEP as u64 + 4);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn same_second_cleans_get_unique_ids() {
        let d = tmp("ids");
        let p = d.join("undo.json");
        record_to(&p, vec![batch("a", 1, &[("/x", Some("/t"))])], "cli", 50).unwrap();
        record_to(&p, vec![batch("b", 1, &[("/y", Some("/u"))])], "cli", 50).unwrap();
        let ids: Vec<u64> = load_from(&p).cleans.iter().map(|c| c.at).collect();
        assert_eq!(ids, vec![50, 51]);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn restore_marks_entries_and_reports_failures() {
        let d = tmp("restore");
        let p = d.join("undo.json");
        record_to(
            &p,
            vec![
                batch("ok", 100, &[("/a1", Some("t1")), ("/a2", Some("t2"))]),
                batch("partly", 50, &[("/b1", Some("t3")), ("/b2", Some("gone"))]),
                batch("taken", 25, &[("/c1", Some("busy"))]),
            ],
            "gui",
            7,
        )
        .unwrap();
        let calls = RefCell::new(Vec::new());
        let put_back = |o: &Path, h: &str| -> Result<()> {
            calls.borrow_mut().push(o.to_path_buf());
            match h {
                "gone" => anyhow::bail!("it's no longer in the Trash"),
                "busy" => anyhow::bail!("something new is already at that path"),
                _ => Ok(()),
            }
        };
        let out = restore_from(&p, 7, &|_, _| {}, &put_back).unwrap();
        assert_eq!(out.restored, 3);
        assert_eq!(out.bytes, 100, "only fully restored items count");
        assert_eq!(out.skipped.len(), 2);

        // A second undo retries only what can still come back ("busy").
        calls.borrow_mut().clear();
        let clean = load_from(&p).cleans.remove(0);
        assert!(clean.can_restore());
        assert_eq!(clean.pending_size(), 25);
        restore_from(&p, 7, &|_, _| {}, &put_back).unwrap();
        assert_eq!(*calls.borrow(), vec![PathBuf::from("/c1")]);

        assert!(restore_from(&p, 999, &|_, _| {}, &put_back).is_err());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn trash_and_restore_round_trip() {
        // Real Trash: only where a desktop trash exists (macOS, or Linux with
        // a home folder). Uses a uniquely named file so it can't clash.
        if std::env::var_os("CI").is_some() && !cfg!(target_os = "macos") {
            return;
        }
        let d = tmp("real");
        let f = d.join(format!("diskdiet-undo-test-{}.txt", now_secs()));
        fs::write(&f, b"hello").unwrap();
        let mut t = Vec::new();
        trashcan::move_to_trash(std::slice::from_ref(&f), &mut t).unwrap();
        assert!(!f.exists());
        let handle = t[0].handle.clone().expect("trash location");
        trashcan::restore(&f, &handle).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"hello");
        fs::remove_dir_all(d).unwrap();
    }
}
