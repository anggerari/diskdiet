//! The public surface that scripts, scheduled jobs and config files depend
//! on. See docs/STABILITY.md. If one of these tests fails, you're about to
//! break someone's setup: keep the old name working (as a hidden alias) or
//! make it a deliberate, documented change.

use std::path::Path;
use std::process::{Command, Output};

use diskdiet::config::{Config, TEMPLATE};

fn diskdiet(args: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_diskdiet"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .output()
        .expect("run diskdiet")
}

fn sandbox(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("diskdiet-compat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// Every subcommand and flag documented in docs/STABILITY.md.
const SURFACE: &[(&[&str], &[&str])] = &[
    (
        &["scan"],
        &[
            "--json",
            "--older-than",
            "--min-size",
            "--no-projects",
            "--no-caches",
            "--no-docker",
            "--big",
            "--no-git",
        ],
    ),
    (
        &["clean"],
        &[
            "--yes",
            "--dry-run",
            "--trash",
            "--tui",
            "--wipe-caches",
            "--docker-all",
            "--docker-volumes",
            "--older-than",
            "--min-size",
            "--no-projects",
            "--no-caches",
            "--no-docker",
            "--big",
            "--no-git",
        ],
    ),
    (
        &["big"],
        &["--min-size", "--yes", "--dry-run", "--trash", "--tui"],
    ),
    (
        &["docker"],
        &[
            "--prune",
            "--yes",
            "--dry-run",
            "--docker-all",
            "--docker-volumes",
        ],
    ),
    (&["alert"], &["--below", "--no-notify"]),
    (&["stats"], &["--json"]),
    (&["config", "init"], &[]),
    (&["config", "path"], &[]),
    (&["config", "show"], &[]),
    (
        &["service", "install"],
        &[
            "--every",
            "--older-than",
            "--with-docker",
            "--no-caches",
            "--trash",
            "--alert-below",
            "--alert-every",
        ],
    ),
    (&["service", "uninstall"], &[]),
    (&["service", "status"], &[]),
];

#[test]
fn commands_and_flags_keep_their_names() {
    let home = sandbox("help");
    for (cmd, flags) in SURFACE {
        let mut args = cmd.to_vec();
        args.push("--help");
        let out = diskdiet(&args, &home);
        assert!(out.status.success(), "`diskdiet {}` is gone", cmd.join(" "));
        let help = String::from_utf8_lossy(&out.stdout);
        for f in *flags {
            assert!(
                help.contains(&format!("{f} "))
                    || help.contains(&format!("{f}\n"))
                    || help.contains(&format!("{f}="))
                    || help.contains(&format!("{f} <")),
                "`diskdiet {}` lost {f}",
                cmd.join(" ")
            );
        }
    }
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn every_config_key_still_parses() {
    let cfg: Config = toml::from_str(
        r#"
        paths = ["~/code"]
        protected = ["~/code/keep"]
        exclude = ["keep-me"]
        older_than = "14d"
        min_size = "10MB"
        trash = true
        picker = "tui"
        smart_caches = false
        skip_caches = ["Maven repository"]
        big_min_size = "500MB"
        big_paths = ["~/Downloads"]

        [[rules]]
        dir = "dist"
        markers = ["vite.config.ts"]
        kind = "Vite"
        "#,
    )
    .expect("a documented config key was removed or renamed");
    assert!(cfg.trash);
    assert_eq!(cfg.rules[0].kind, "Vite");
}

#[test]
fn the_config_template_parses_when_uncommented() {
    let uncommented: String = TEMPLATE
        .lines()
        .map(|l| l.strip_prefix("# ").unwrap_or(l))
        .filter(|l| {
            !l.starts_with('#') && (l.contains(" = ") || l.starts_with("[[") || l.is_empty())
        })
        .collect::<Vec<_>>()
        .join("\n");
    toml::from_str::<Config>(&uncommented).expect("config template");
}

#[test]
fn scan_json_keeps_its_fields() {
    let home = sandbox("json");
    std::fs::create_dir_all(home.join("code/app/node_modules/dep")).unwrap();
    std::fs::write(home.join("code/app/package.json"), "{}").unwrap();
    std::fs::write(
        home.join("code/app/node_modules/dep/i.js"),
        vec![b'x'; 4096],
    )
    .unwrap();
    let out = diskdiet(
        &[
            "scan",
            home.join("code").to_str().unwrap(),
            "--json",
            "--no-caches",
            "--no-docker",
            "--min-size",
            "0B",
        ],
        &home,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for k in ["version", "free_space", "reclaimable", "items"] {
        assert!(v.get(k).is_some(), "scan --json lost `{k}`");
    }
    let item = &v["items"][0];
    for k in [
        "category",
        "kind",
        "label",
        "path",
        "size",
        "age_days",
        "note",
        "default_selected",
        "git",
    ] {
        assert!(item.get(k).is_some(), "scan --json items lost `{k}`");
    }
    assert_eq!(item["category"], "project");
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn stats_json_keeps_its_fields() {
    let home = sandbox("stats");
    let out = diskdiet(&["stats", "--json"], &home);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for k in [
        "total_freed",
        "total_trashed",
        "items_removed",
        "runs",
        "history",
    ] {
        assert!(v.get(k).is_some(), "stats --json lost `{k}`");
    }
    let _ = std::fs::remove_dir_all(home);
}
