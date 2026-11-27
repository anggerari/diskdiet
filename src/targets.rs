//! What counts as junk: per-project build/dependency folders and global caches.

use std::fs;
use std::path::Path;

/// How we confirm a folder really is a generated artifact (and not, say,
/// a hand-written folder that happens to be called `build`).
pub enum Marker {
    /// Always safe to delete wherever it appears.
    Any,
    /// The parent folder must contain one of these files.
    Files(&'static [&'static str]),
    /// The parent folder must contain a file with this extension.
    Ext(&'static str),
}

impl Marker {
    pub fn matches(&self, parent: &Path) -> bool {
        match self {
            Marker::Any => true,
            Marker::Files(files) => files.iter().any(|f| parent.join(f).exists()),
            Marker::Ext(ext) => fs::read_dir(parent)
                .map(|rd| {
                    rd.flatten()
                        .any(|e| e.path().extension().is_some_and(|x| x == *ext))
                })
                .unwrap_or(false),
        }
    }
}

pub struct ArtifactRule {
    pub dir: &'static str,
    pub marker: Marker,
    pub kind: &'static str,
}

const NODE: &[&str] = &["package.json"];
const GRADLE: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];
const PYTHON: &[&str] = &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"];

macro_rules! rule {
    ($dir:expr, $marker:expr, $kind:expr) => {
        ArtifactRule {
            dir: $dir,
            marker: $marker,
            kind: $kind,
        }
    };
}

pub const RULES: &[ArtifactRule] = &[
    // JavaScript / TypeScript
    rule!("node_modules", Marker::Files(NODE), "Node"),
    rule!(".next", Marker::Files(NODE), "Next.js"),
    rule!(".nuxt", Marker::Files(NODE), "Nuxt"),
    rule!(
        ".output",
        Marker::Files(&["nuxt.config.ts", "nuxt.config.js"]),
        "Nuxt"
    ),
    rule!(".svelte-kit", Marker::Files(NODE), "SvelteKit"),
    rule!(".turbo", Marker::Files(NODE), "Turborepo"),
    rule!(".parcel-cache", Marker::Files(NODE), "Parcel"),
    rule!(".angular", Marker::Files(&["angular.json"]), "Angular"),
    // Rust / JVM
    rule!("target", Marker::Files(&["Cargo.toml"]), "Rust"),
    rule!("target", Marker::Files(&["pom.xml"]), "Maven"),
    rule!(".gradle", Marker::Files(GRADLE), "Gradle"),
    rule!("build", Marker::Files(GRADLE), "Gradle"),
    // Mobile
    rule!("build", Marker::Files(&["pubspec.yaml"]), "Flutter"),
    rule!(".dart_tool", Marker::Files(&["pubspec.yaml"]), "Flutter"),
    rule!("Pods", Marker::Files(&["Podfile"]), "CocoaPods"),
    rule!(".build", Marker::Files(&["Package.swift"]), "SwiftPM"),
    // Python
    rule!(".venv", Marker::Files(PYTHON), "Python venv"),
    rule!("venv", Marker::Files(PYTHON), "Python venv"),
    rule!("__pycache__", Marker::Any, "Python"),
    rule!(".pytest_cache", Marker::Any, "Python"),
    rule!(".mypy_cache", Marker::Any, "Python"),
    rule!(".ruff_cache", Marker::Any, "Python"),
    // Others
    rule!(".zig-cache", Marker::Files(&["build.zig"]), "Zig"),
    rule!("zig-cache", Marker::Files(&["build.zig"]), "Zig"),
    rule!(".stack-work", Marker::Files(&["stack.yaml"]), "Haskell"),
    rule!("_build", Marker::Files(&["mix.exs"]), "Elixir"),
    rule!("deps", Marker::Files(&["mix.exs"]), "Elixir"),
    rule!("elm-stuff", Marker::Files(&["elm.json"]), "Elm"),
    rule!(".terraform", Marker::Ext("tf"), "Terraform"),
];

pub fn is_artifact_name(name: &str) -> bool {
    RULES.iter().any(|r| r.dir == name)
}

/// Global caches, relative to $HOME. Everything here is re-downloadable.
pub struct CacheDef {
    pub name: &'static str,
    pub paths: &'static [&'static str],
    /// A command that prunes only stale entries, used instead of wiping when available.
    pub smart: Option<&'static [&'static str]>,
}

pub const CACHES: &[CacheDef] = &[
    CacheDef {
        name: "npm cache",
        paths: &[".npm/_cacache"],
        smart: None,
    },
    CacheDef {
        name: "Yarn cache",
        paths: &["Library/Caches/Yarn", ".cache/yarn", ".yarn/berry/cache"],
        smart: None,
    },
    CacheDef {
        name: "pnpm store",
        paths: &[
            "Library/pnpm/store",
            ".local/share/pnpm/store",
            ".pnpm-store",
        ],
        smart: Some(&["pnpm", "store", "prune"]),
    },
    CacheDef {
        name: "Bun cache",
        paths: &[".bun/install/cache"],
        smart: None,
    },
    CacheDef {
        name: "node-gyp",
        paths: &["Library/Caches/node-gyp", ".cache/node-gyp"],
        smart: None,
    },
    CacheDef {
        name: "Cargo registry",
        paths: &[
            ".cargo/registry/cache",
            ".cargo/registry/src",
            ".cargo/git/checkouts",
        ],
        smart: None,
    },
    CacheDef {
        name: "pip cache",
        paths: &["Library/Caches/pip", ".cache/pip"],
        smart: None,
    },
    CacheDef {
        name: "uv cache",
        paths: &["Library/Caches/uv", ".cache/uv"],
        smart: Some(&["uv", "cache", "prune"]),
    },
    CacheDef {
        name: "Go build cache",
        paths: &["Library/Caches/go-build", ".cache/go-build"],
        smart: None,
    },
    CacheDef {
        name: "Gradle caches",
        paths: &[".gradle/caches"],
        smart: None,
    },
    CacheDef {
        name: "Maven repository",
        paths: &[".m2/repository"],
        smart: None,
    },
    CacheDef {
        name: "Composer cache",
        paths: &["Library/Caches/composer", ".cache/composer"],
        smart: None,
    },
    CacheDef {
        name: "Xcode DerivedData",
        paths: &["Library/Developer/Xcode/DerivedData"],
        smart: None,
    },
    CacheDef {
        name: "Xcode device support",
        paths: &["Library/Developer/Xcode/iOS DeviceSupport"],
        smart: None,
    },
    CacheDef {
        name: "Simulator caches",
        paths: &["Library/Developer/CoreSimulator/Caches"],
        smart: None,
    },
    CacheDef {
        name: "CocoaPods cache",
        paths: &["Library/Caches/CocoaPods"],
        smart: None,
    },
    CacheDef {
        name: "Homebrew cache",
        paths: &["Library/Caches/Homebrew", ".cache/Homebrew"],
        smart: Some(&["brew", "cleanup", "--prune=all"]),
    },
    CacheDef {
        name: "Playwright browsers",
        paths: &["Library/Caches/ms-playwright", ".cache/ms-playwright"],
        smart: None,
    },
    CacheDef {
        name: "Puppeteer browsers",
        paths: &[".cache/puppeteer"],
        smart: None,
    },
    CacheDef {
        name: "Electron cache",
        paths: &["Library/Caches/electron", ".cache/electron"],
        smart: None,
    },
    CacheDef {
        name: "JetBrains caches",
        paths: &["Library/Caches/JetBrains", ".cache/JetBrains"],
        smart: None,
    },
    CacheDef {
        name: "Trash",
        paths: &[".Trash", ".local/share/Trash"],
        smart: None,
    },
];
