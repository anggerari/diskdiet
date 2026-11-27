//! Running total of space freed, stored as JSON in the user data folder.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::util::now_secs;

const MAX_HISTORY: usize = 200;

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct Stats {
    pub total_freed: u64,
    pub total_trashed: u64,
    pub items_removed: u64,
    pub runs: u64,
    pub first_run: Option<u64>,
    pub last_run: Option<u64>,
    pub history: Vec<Run>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Run {
    /// Unix seconds.
    pub at: u64,
    pub freed: u64,
    pub trashed: u64,
    pub items: u64,
    /// "cli", "service" or "gui".
    pub source: String,
}

pub fn path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("diskdiet/stats.json"))
}

pub fn load() -> Stats {
    path()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn record(freed: u64, trashed: u64, items: u64, source: &str) -> Result<()> {
    if items == 0 {
        return Ok(());
    }
    let Some(p) = path() else { return Ok(()) };
    let mut s = load();
    let at = now_secs();
    s.total_freed += freed;
    s.total_trashed += trashed;
    s.items_removed += items;
    s.runs += 1;
    s.first_run.get_or_insert(at);
    s.last_run = Some(at);
    s.history.push(Run {
        at,
        freed,
        trashed,
        items,
        source: source.into(),
    });
    if s.history.len() > MAX_HISTORY {
        let extra = s.history.len() - MAX_HISTORY;
        s.history.drain(..extra);
    }
    fs::create_dir_all(p.parent().unwrap())?;
    fs::write(&p, serde_json::to_vec_pretty(&s)?)?;
    Ok(())
}
