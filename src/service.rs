//! Install diskdiet as a scheduled background job:
//! a launchd agent on macOS, a systemd user timer on Linux.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

pub struct ServiceConfig {
    pub every: Duration,
    pub args: Vec<String>,
}

fn exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let s = exe.to_string_lossy();
    if s.contains("/target/debug/") || s.contains("/target/release/") {
        eprintln!(
            "warning: the service will run {s}\n         install the binary first (`cargo install --path .`) so it survives `cargo clean`."
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

    const LABEL: &str = "dev.diskdiet.agent";

    fn plist_path(home: &Path) -> PathBuf {
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    fn log_path(home: &Path) -> PathBuf {
        home.join("Library/Logs/diskdiet.log")
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn install(home: &Path, cfg: &ServiceConfig) -> Result<()> {
        let exe = exe()?;
        let mut args = vec![exe.to_string_lossy().into_owned()];
        args.extend(cfg.args.iter().cloned());
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
  <string>{LABEL}</string>
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
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
            secs = cfg.every.as_secs()
        );
        let path = plist_path(home);
        fs::create_dir_all(path.parent().unwrap())?;
        // Reload if already installed.
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
        println!("Installed launchd agent: {}", path.display());
        println!("Runs: {}", args.join(" "));
        println!("Logs: {}", log_path(home).display());
        Ok(())
    }

    pub fn uninstall(home: &Path) -> Result<()> {
        let path = plist_path(home);
        if !path.exists() {
            println!("Service is not installed.");
            return Ok(());
        }
        let _ = Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&path)
            .output();
        fs::remove_file(&path)?;
        println!("Removed {}", path.display());
        Ok(())
    }

    pub fn status(home: &Path) -> Result<()> {
        let path = plist_path(home);
        if !path.exists() {
            println!("Service is not installed. Run `diskdiet service install`.");
            return Ok(());
        }
        let out = Command::new("launchctl").args(["list", LABEL]).output()?;
        let loaded = out.status.success();
        println!("Plist:  {}", path.display());
        println!("Loaded: {}", if loaded { "yes" } else { "no" });
        println!("Logs:   {}", log_path(home).display());
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

    pub fn install(home: &Path, cfg: &ServiceConfig) -> Result<()> {
        let exe = exe()?;
        let mut args = vec![exe.to_string_lossy().into_owned()];
        args.extend(cfg.args.iter().cloned());
        let cmd: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
        let dir = unit_dir(home);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("diskdiet.service"),
            format!(
                "[Unit]\nDescription=diskdiet disk cleanup\n\n[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\nExecStart={}\n",
                cmd.join(" ")
            ),
        )?;
        fs::write(
            dir.join("diskdiet.timer"),
            format!(
                "[Unit]\nDescription=Run diskdiet periodically\n\n[Timer]\nOnBootSec=15min\nOnUnitActiveSec={}s\n\n[Install]\nWantedBy=timers.target\n",
                cfg.every.as_secs()
            ),
        )?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", "diskdiet.timer"])?;
        println!("Installed systemd user timer in {}", dir.display());
        println!("Runs: {}", args.join(" "));
        println!("Logs: journalctl --user -u diskdiet.service");
        Ok(())
    }

    pub fn uninstall(home: &Path) -> Result<()> {
        let _ = systemctl(&["disable", "--now", "diskdiet.timer"]);
        let dir = unit_dir(home);
        let _ = fs::remove_file(dir.join("diskdiet.service"));
        let _ = fs::remove_file(dir.join("diskdiet.timer"));
        let _ = systemctl(&["daemon-reload"]);
        println!("Service removed.");
        Ok(())
    }

    pub fn status(_home: &Path) -> Result<()> {
        let _ = Command::new("systemctl")
            .args(["--user", "list-timers", "diskdiet.timer"])
            .status();
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::*;
    use anyhow::bail;

    pub fn install(_: &Path, _: &ServiceConfig) -> Result<()> {
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
