//! Edge cases a cleanup tool must get right. Each test builds a small fake
//! home in a temp folder, scans it with the real engine and (where it makes
//! sense) deletes for real, then checks that nothing else was touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use diskdiet::config::Config;
use diskdiet::engine::{self, AutoPolicy, DeleteOptions, Item, Options, What};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("diskdiet-safety-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        // Canonical, so paths compare equal to what the scanner reports
        // (/tmp is a symlink on macOS).
        Sandbox(d.canonicalize().unwrap())
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn file(&self, rel: &str, bytes: usize) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![b'x'; bytes]).unwrap();
        p
    }

    /// A Node project with a populated node_modules.
    fn node_project(&self, rel: &str) -> PathBuf {
        self.file(&format!("{rel}/package.json"), 2);
        self.file(&format!("{rel}/node_modules/dep/index.js"), 4096);
        self.path(&format!("{rel}/node_modules"))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Restore permissions first so cleanup can't fail on a locked folder.
        let _ = Command::new("chmod")
            .arg("-R")
            .arg("u+rwx")
            .arg(&self.0)
            .status();
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn options(root: &Path) -> Options {
    let mut o = Options::from_config(&Config::default(), root);
    o.paths = vec![root.to_path_buf()];
    o.min_size = 0;
    o.older_than = None;
    o.caches = false;
    o.docker = false;
    o.big = false;
    o.git = false;
    o
}

fn scan(root: &Path, opts: &Options) -> Vec<Item> {
    engine::gather(opts, &Config::default(), root, &|_| {}).items
}

fn artifact_paths(items: &[Item]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = items
        .iter()
        .filter_map(|i| match &i.what {
            What::Artifact { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

fn delete(items: &[Item], home: &Path) -> engine::Outcome {
    let refs: Vec<&Item> = items.iter().collect();
    engine::delete(
        &refs,
        home,
        DeleteOptions {
            trash: false,
            smart_caches: false,
        },
        &|_, _| {},
    )
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(unix)]
#[test]
fn symlinked_folders_are_not_followed_while_scanning() {
    let home = Sandbox::new("symscan");
    let outside = Sandbox::new("symscan-outside");
    outside.node_project("elsewhere");
    // A link from the scanned tree to a project outside it.
    symlink(
        &outside.path("elsewhere"),
        &home.path("code/link-to-elsewhere"),
    );
    // A pnpm-style node_modules that is itself a symlink.
    home.file("code/linked/package.json", 2);
    symlink(
        &outside.path("elsewhere/node_modules"),
        &home.path("code/linked/node_modules"),
    );
    let real = home.node_project("code/real");

    let items = scan(&home.0, &options(&home.0));
    assert_eq!(artifact_paths(&items), vec![real]);
}

#[cfg(unix)]
#[test]
fn deleting_never_follows_symlinks_out_of_the_folder() {
    let home = Sandbox::new("symdel");
    let outside = Sandbox::new("symdel-outside");
    let precious = outside.file("precious/data.db", 1000);
    let nm = home.node_project("app");
    // node_modules contains links to a folder and a file outside it
    // (npm link / pnpm do this).
    symlink(&outside.path("precious"), &nm.join("linked-dir"));
    symlink(&precious, &nm.join("linked-file"));

    let items = scan(&home.0, &options(&home.0));
    // The link targets don't count toward the size either.
    assert!(
        items[0].size < 64 * 1024,
        "size {} counted link targets",
        items[0].size
    );
    let out = delete(&items, &home.0);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(!nm.exists());
    assert_eq!(fs::read(&precious).unwrap().len(), 1000);
}

#[test]
fn odd_names_with_spaces_unicode_and_emoji() {
    let home = Sandbox::new("names");
    let a = home.node_project("My Projects/cool app");
    let b = home.node_project("código/プロジェクト 🚀");
    let c = home.node_project("quotes 'n \"stuff\" & $HOME");
    let items = scan(&home.0, &options(&home.0));
    let mut want = vec![a.clone(), b.clone(), c.clone()];
    want.sort();
    assert_eq!(artifact_paths(&items), want);

    let out = delete(&items, &home.0);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    for p in [a, b, c] {
        assert!(!p.exists());
        assert!(
            p.parent().unwrap().join("package.json").exists(),
            "project kept"
        );
    }
}

#[cfg(unix)]
#[test]
fn unreadable_folders_are_skipped_not_fatal() {
    if is_root() {
        return; // root can read everything; nothing to test
    }
    let home = Sandbox::new("perms");
    let ok = home.node_project("ok");
    home.node_project("locked/inner");
    chmod(&home.path("locked"), "000");

    let items = scan(&home.0, &options(&home.0));
    assert_eq!(artifact_paths(&items), vec![ok]);
}

#[cfg(unix)]
#[test]
fn a_failed_delete_is_reported_and_the_rest_still_goes() {
    if is_root() {
        return;
    }
    let home = Sandbox::new("partial");
    let stuck = home.node_project("stuck");
    let fine = home.node_project("fine");
    let items = scan(&home.0, &options(&home.0));
    assert_eq!(items.len(), 2);
    // Something else locks a folder between the scan and the clean.
    chmod(&stuck.join("dep"), "500");

    let out = delete(&items, &home.0);
    assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
    assert!(out.errors[0].contains("stuck"), "{:?}", out.errors);
    assert_eq!(out.removed, 1);
    assert!(!fine.exists());
}

#[test]
fn items_that_vanished_since_the_scan_are_an_error_not_a_crash() {
    let home = Sandbox::new("vanished");
    let nm = home.node_project("app");
    let items = scan(&home.0, &options(&home.0));
    fs::remove_dir_all(&nm).unwrap();
    let out = delete(&items, &home.0);
    assert_eq!(out.errors.len(), 1);
    assert_eq!(out.freed, 0);
}

#[test]
fn a_missing_scan_folder_is_a_warning() {
    // e.g. an external drive that was unplugged since the config was saved.
    let home = Sandbox::new("unplugged");
    let nm = home.node_project("code/app");
    let mut o = options(&home.0);
    o.paths = vec![home.path("Volumes/Backup/code"), home.path("code")];
    let s = engine::gather(&o, &Config::default(), &home.0, &|_| {});
    assert_eq!(artifact_paths(&s.items), vec![nm]);
    assert_eq!(s.warnings.len(), 1);
    assert!(s.warnings[0].contains("not found"), "{:?}", s.warnings);
}

#[test]
fn recently_touched_projects_are_left_alone_with_older_than() {
    // A project you have open in an editor keeps getting touched.
    let home = Sandbox::new("recent");
    let active = home.node_project("active");
    let stale = home.node_project("stale");
    let old = SystemTime::now() - Duration::from_secs(90 * 86_400);
    fs::File::options()
        .write(true)
        .open(stale.parent().unwrap().join("package.json"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    // Fresh files *inside* node_modules don't make a project look active.
    home.file("stale/node_modules/dep/new.js", 10);

    let mut o = options(&home.0);
    o.older_than = Some(Duration::from_secs(30 * 86_400));
    let items = scan(&home.0, &o);
    assert_eq!(artifact_paths(&items), vec![stale]);
    assert!(active.exists());
}

#[test]
fn protected_and_excluded_paths_never_show_up() {
    let home = Sandbox::new("protected");
    home.node_project("code/keep");
    home.node_project("code/vendor-lock/x");
    let gone = home.node_project("code/app");
    let cfg: Config = toml::from_str(&format!(
        "protected = [{:?}]\nexclude = [\"vendor-lock\"]",
        home.path("code/keep").display().to_string()
    ))
    .unwrap();
    let items = engine::gather(&options(&home.0), &cfg, &home.0, &|_| {}).items;
    assert_eq!(artifact_paths(&items), vec![gone]);
}

#[test]
fn hand_made_build_folders_are_not_artifacts() {
    let home = Sandbox::new("markers");
    home.file("notes/build/important.txt", 10);
    home.file("misc/target/keep.txt", 10);
    home.file("lib/.venv-not/x", 10);
    home.file("app/node_modules/x.js", 10); // no package.json next to it
    assert!(scan(&home.0, &options(&home.0)).is_empty());
}

#[test]
fn dirty_git_projects_need_a_human() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let home = Sandbox::new("git");
    let clean = home.node_project("clean");
    let dirty = home.node_project("dirty");
    for p in ["clean", "dirty"] {
        let dir = home.path(p);
        fs::write(dir.join(".gitignore"), "node_modules\n").unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["add", "."]);
        git(
            &dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ],
        );
    }
    fs::write(home.path("dirty/package.json"), "{\"changed\":1}").unwrap();

    let mut o = options(&home.0);
    o.git = true;
    let items = scan(&home.0, &o);
    let find = |p: &Path| {
        items
            .iter()
            .find(|i| i.reveal_path() == Some(p))
            .expect("listed")
    };
    let policy = AutoPolicy::default();
    assert!(find(&clean).default_selected() && find(&clean).auto_selected(policy));
    assert!(
        !find(&dirty).default_selected(),
        "dirty project pre-selected"
    );
    assert!(
        !find(&dirty).auto_selected(policy),
        "dirty project removed by --yes"
    );
}

#[test]
fn big_files_are_never_removed_automatically() {
    let home = Sandbox::new("bigauto");
    home.file("Downloads/installer.dmg", 2048);
    let mut o = options(&home.0);
    o.projects = false;
    o.big = true;
    o.big_paths = vec![home.path("Downloads")];
    o.big_min_size = 1024;
    let items = scan(&home.0, &o);
    assert_eq!(items.len(), 1);
    assert!(!items[0].default_selected());
    assert!(!items[0].auto_selected(AutoPolicy {
        docker_all: true,
        docker_volumes: true
    }));
}

#[cfg(unix)]
fn chmod(p: &Path, mode: &str) {
    assert!(Command::new("chmod")
        .arg(mode)
        .arg(p)
        .status()
        .unwrap()
        .success());
}

#[cfg(unix)]
fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}
