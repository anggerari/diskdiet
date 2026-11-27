//! Low free-space desktop notifications.

use std::path::Path;
use std::process::Command;

use anyhow::Result;

use crate::util::{free_space, human_bytes};

pub fn notify(title: &str, body: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"{}\" sound name \"Funk\"",
            esc(body),
            esc(title)
        );
        Command::new("osascript").args(["-e", &script]).output()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new("notify-send")
            .args(["--app-name=diskdiet", title, body])
            .output()?;
    }
    Ok(())
}

pub enum Check {
    Ok(u64),
    Low(u64),
}

/// Compare free space against `below` and notify when it is low.
pub fn check(home: &Path, below: u64, send: bool) -> Result<Check> {
    let free = free_space(home).ok_or_else(|| anyhow::anyhow!("could not read free space"))?;
    if free >= below {
        return Ok(Check::Ok(free));
    }
    if send {
        notify(
            "diskdiet: storage is running low",
            &format!(
                "Only {} free (alert below {}). Run `diskdiet clean` or open diskdiet.",
                human_bytes(free),
                human_bytes(below)
            ),
        )?;
    }
    Ok(Check::Low(free))
}
