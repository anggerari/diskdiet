//! Docker missing, stopped or hanging must never break or stall a scan.
//! Own test binary: it changes PATH, which is process-wide.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use diskdiet::config::Config;
use diskdiet::engine::{self, Options};

#[test]
fn scans_survive_any_kind_of_broken_docker() {
    let dir = std::env::temp_dir().join(format!("diskdiet-docker-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("docker");
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let set_docker = |script: Option<&str>| {
        let _ = fs::remove_file(&fake);
        if let Some(s) = script {
            fs::write(&fake, format!("#!/bin/sh\n{s}\n")).unwrap();
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        }
    };
    // Only the fake docker (or none) on PATH. The fakes use absolute
    // #!/bin/sh, and a real docker in /usr/bin must not be found.
    std::env::set_var("PATH", &dir);

    let mut opts = Options::from_config(&Config::default(), &dir);
    opts.projects = false;
    opts.caches = false;
    opts.docker = true;

    // Not installed.
    set_docker(None);
    let s = engine::gather(&opts, &Config::default(), &dir, &|_| {});
    assert!(s.items.is_empty());

    // Installed, daemon not running.
    set_docker(Some(
        "echo 'Cannot connect to the Docker daemon' >&2; exit 1",
    ));
    let s = engine::gather(&opts, &Config::default(), &dir, &|_| {});
    assert!(s.items.is_empty());

    // Daemon hangs (Docker Desktop starting up): give up instead of waiting.
    set_docker(Some("exec /bin/sleep 30"));
    let t = Instant::now();
    assert!(!diskdiet::docker::available_within(Duration::from_millis(
        500
    )));
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "waited {:?}",
        t.elapsed()
    );

    // Answers `info`, but prints garbage for the rest.
    set_docker(Some("[ \"$1\" = info ] && exit 0; echo 'not json'"));
    let s = engine::gather(&opts, &Config::default(), &dir, &|_| {});
    assert!(s.items.is_empty());

    std::env::set_var("PATH", old_path);
    let _ = fs::remove_dir_all(&dir);
}
