//! diskdiet: reclaim storage from dev junk. Shared by the CLI and the desktop app.

pub mod alert;
pub mod bigfiles;
pub mod config;
pub mod docker;
pub mod engine;
pub mod git;
pub mod scan;
pub mod service;
pub mod stats;
pub mod targets;
pub mod trashcan;
pub mod undo;
pub mod update;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
