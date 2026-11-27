//! Scheduled background jobs: a launchd agent on macOS, systemd user timers on
//! Linux. Two jobs are supported: `clean` (periodic cleanup) and `alert`
//! (low free-space notification).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

pub struct Job {
    /// "clean" or "alert".
    pub name: &'static str,
    pub every: Duration,
    /// Arguments after the executable.
    pub args: Vec<String>,
}

pub const JOB_NAMES: &[&str] = &["clean", "alert"];

fn exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let s = exe.to_string_lossy();
    if s.contains("/target/debug/") || s.contains("/target/release/") {
        eprintln!(
            "warning: the service will run {s}\n         install the binary first (`make install`) so it survives `cargo clean`."
        );
    }
    Ok(exe)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use anyhow::{bail, Context};
    use std::fs;
    use std::process::Command;

    fn label(job: &str) -> String {
        // Keep the original label for the cleanup job so upgrades replace it.
        if job == "clean" {
            "dev.diskdiet.agent".into()
        } else {
            format!("dev.diskdiet.{job}")
        }
    }

    fn plist_path(home: &Path, job: &str) -> PathBuf {
        home.join("Library/LaunchAgents")
            .join(format!("{}.plist", label(job)))
    }

    fn log_path(home: &Path) -> PathBuf {
        home.join("Library/Logs/diskdiet.log")
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn install(home: &Path, job: &Job) -> Result<()> {
        let exe = exe()?;
        let mut args = vec![exe.to_string_lossy().into_owned()];
        args.extend(job.args.iter().cloned());
        let args_xml: String = args
            .iter()
            .map(|a| format!("\n    <string>{}</string>", xml_escape(a)))
            .collect();
        let log = xml_escape(&log_path(home).to_string_lossy());
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>{args_xml}
  </array>
  <key>StartInterval</key>
  <integer>{secs}</integer>
  <key>RunAtLoad</key>
  <false/>
  <key>LowPriorityIO</key>
  <true/>
  <key>Nice</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
            label = label(job.name),
            secs = job.every.as_secs()
        );
        let path = plist_path(home, job.name);
        fs::create_dir_all(path.parent().unwrap())?;
        let _ = Command::new("launchctl").arg("unload").arg(&path).output();
        fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))?;
        let out = Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&path)
            .output()?;
        if !out.status.success() {
            bail!(
                "launchctl load failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        println!("Installed '{}' job: {}", job.name, path.display());
        println!("  Runs: {}", args.join(" "));
        println!("  Logs: {}", log_path(home).display());
        Ok(())
    }

    pub fn uninstall(home: &Path) -> Result<()> {
        let mut any = false;
        for job in JOB_NAMES {
            let path = plist_path(home, job);
            if path.exists() {
                let _ = Command::new("launchctl")
                    .args(["unload", "-w"])
                    .arg(&path)
                    .output();
                fs::remove_file(&path)?;
                println!("Removed {}", path.display());
                any = true;
            }
        }
        if !any {
            println!("Service is not installed.");
        }
        Ok(())
    }

    pub fn status(home: &Path) -> Result<()> {
        for job in JOB_NAMES {
            let path = plist_path(home, job);
            if !path.exists() {
                println!("{job:<6} not installed");
                continue;
            }
            let loaded = Command::new("launchctl")
                .args(["list", &label(job)])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            println!(
                "{job:<6} installed, {} ({})",
                if loaded { "loaded" } else { "NOT loaded" },
                path.display()
            );
        }
        println!("Logs: {}", log_path(home).display());
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use anyhow::bail;
    use std::fs;
    use std::process::Command;

    fn unit_dir(home: &Path) -> PathBuf {
        home.join(".config/systemd/user")
    }

    fn unit(job: &str) -> String {
        if job == "clean" {
            "diskdiet".into()
        } else {
            format!("diskdiet-{job}")
        }
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let out = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()?;
        if !out.status.success() {
            bail!(
                "systemctl --user {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    fn shell_quote(s: &str) -> String {
        if s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=".contains(c))
        {
            s.to_string()
        } else {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }
    }

    pub fn install(home: &Path, job: &Job) -> Result<()> {
        let exe = exe()?;
        let mut args = vec![exe.to_string_lossy().into_owned()];
        args.extend(job.args.iter().cloned());
        let cmd: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
        let dir = unit_dir(home);
        let name = unit(job.name);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join(format!("{name}.service")),
            format!(
                "[Unit]\nDescription=diskdiet {}\n\n[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\nExecStart={}\n",
                job.name,
                cmd.join(" ")
            ),
        )?;
        fs::write(
            dir.join(format!("{name}.timer")),
            format!(
                "[Unit]\nDescription=Run diskdiet {} periodically\n\n[Timer]\nOnBootSec=15min\nOnUnitActiveSec={}s\n\n[Install]\nWantedBy=timers.target\n",
                job.name,
                job.every.as_secs()
            ),
        )?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", &format!("{name}.timer")])?;
        println!("Installed '{}' job in {}", job.name, dir.display());
        println!("  Runs: {}", args.join(" "));
        println!("  Logs: journalctl --user -u {name}.service");
        Ok(())
    }

    pub fn uninstall(home: &Path) -> Result<()> {
        let dir = unit_dir(home);
        for job in JOB_NAMES {
            let name = unit(job);
            let _ = systemctl(&["disable", "--now", &format!("{name}.timer")]);
            let _ = fs::remove_file(dir.join(format!("{name}.service")));
            let _ = fs::remove_file(dir.join(format!("{name}.timer")));
        }
        let _ = systemctl(&["daemon-reload"]);
        println!("Service removed.");
        Ok(())
    }

    pub fn status(_home: &Path) -> Result<()> {
        let _ = Command::new("systemctl")
            .args(["--user", "list-timers", "diskdiet*"])
            .status();
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::*;
    use anyhow::bail;

    pub fn install(_: &Path, _: &Job) -> Result<()> {
        let _ = exe();
        bail!("background service is only supported on macOS and Linux")
    }
    pub fn uninstall(_: &Path) -> Result<()> {
        bail!("background service is only supported on macOS and Linux")
    }
    pub fn status(_: &Path) -> Result<()> {
        bail!("background service is only supported on macOS and Linux")
    }
}

pub use imp::{install, status, uninstall};
