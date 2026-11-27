//! diskdiet desktop app (beta).
//!
//! A native window on top of the same engine as the CLI. Design rules: look
//! like a system utility; neutral colors that follow the OS theme; color only
//! where it carries meaning (one accent, red for destructive, amber for
//! warnings, small ecosystem badges).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icons;
mod theme;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Modifiers, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, Vec2,
};
use egui_extras::{Column, TableBuilder};

use diskdiet::config::{self, Config};
use diskdiet::docker::{Kind as DockerKind, Tier};
use diskdiet::engine::{self, Category, DeleteOptions, Item, Options, Outcome, What};
use diskdiet::util::{disk_space, expand_tilde, has_command, human_bytes, now_secs, pretty_path};
use diskdiet::{stats, VERSION};
use icons::Icon;
use theme::Palette;

const CATEGORIES: [Category; 4] = [
    Category::Project,
    Category::Cache,
    Category::Docker,
    Category::Big,
];

fn main() -> eframe::Result {
    diskdiet::util::augment_path();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("diskdiet")
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([860.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "diskdiet",
        options,
        Box::new(|cc| {
            let mut app = App::new(&cc.egui_ctx);
            // `diskdiet-gui --scan [folders...]` starts scanning right away.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if args.first().map(String::as_str) == Some("--scan") {
                if args.len() > 1 {
                    app.paths = args[1..]
                        .iter()
                        .map(|a| pretty_path(Path::new(a), &app.home))
                        .collect();
                }
                app.start_scan(&cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}

// ───────────────────────────── data ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Cleanup(Option<Category>),
    Stats,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Neutral,
    Ok,
    Warn,
    Accent,
}

enum Chip {
    Mono(String, Color32),
    Glyph(Icon, Color32),
}

impl Chip {
    fn color(&self) -> Color32 {
        match self {
            Chip::Mono(_, c) | Chip::Glyph(_, c) => *c,
        }
    }
}

/// Pre-rendered strings for one item so the table doesn't allocate per frame.
struct RowView {
    category: Category,
    kind: String,
    type_label: String,
    chip: Chip,
    name: String,
    detail: String,
    age: String,
    age_days: Option<u64>,
    size: u64,
    size_label: String,
    pills: Vec<(String, Tone)>,
    warn: bool,
    reveal: Option<PathBuf>,
    haystack: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Size,
    Name,
    Age,
}

enum Line {
    Group(Category),
    Item(usize),
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Scanning,
    Deleting,
}

#[derive(Default)]
struct Shared {
    message: String,
    scan: Option<engine::Scan>,
    progress: (usize, usize),
    done: Option<Finished>,
}

struct Finished {
    outcome: Outcome,
    removed: Vec<usize>,
    before: Option<u64>,
}

struct Toast {
    ok: bool,
    title: String,
    body: String,
    errors: Vec<String>,
}

struct App {
    home: PathBuf,
    cfg: Config,
    // settings
    paths: Vec<String>,
    new_path: String,
    include: [bool; 4],
    older_days: u32,
    min_mb: u32,
    git: bool,
    trash: bool,
    smart: bool,
    settings_note: Option<(String, bool)>,
    // results
    items: Arc<Vec<Item>>,
    rows: Vec<RowView>,
    selected: Vec<bool>,
    gone: Vec<bool>,
    scanned: bool,
    warnings: Vec<String>,
    // view state
    view: View,
    kind_filter: Option<String>,
    search: String,
    focus_search: bool,
    sort: (SortKey, bool),
    collapsed: HashSet<Category>,
    pending: Option<Vec<usize>>,
    toast: Option<Toast>,
    // background work
    phase: Phase,
    shared: Arc<Mutex<Shared>>,
    scan_started: Instant,
    // info
    disk: Option<(u64, u64)>,
    stats: stats::Stats,
    /// Dev only: `DISKDIET_SNAPSHOT=out.bmp` saves a screenshot after the
    /// first scan and quits (used to make README images).
    snapshot: Option<(PathBuf, u32)>,
}

// ───────────────────────────── helpers ─────────────────────────────

fn age_words(days: u64) -> String {
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        d if d < 30 => format!("{d} days ago"),
        d if d < 60 => "1 month ago".into(),
        d if d < 365 => format!("{} months ago", d / 30),
        d if d < 730 => "1 year ago".into(),
        d => format!("{} years ago", d / 365),
    }
}

/// Docker's compact ages ("26h", "2w", "3mo") as words.
fn docker_age(s: &str) -> (String, Option<u64>) {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (n, unit) = s.split_at(split);
    let Ok(n) = n.parse::<u64>() else {
        return (s.to_string(), None);
    };
    let days = match unit {
        "m" | "h" => 0,
        "d" => n,
        "w" => n * 7,
        "mo" => n * 30,
        "y" => n * 365,
        _ => return (s.to_string(), None),
    };
    let words = match unit {
        "m" => format!("{n} min ago"),
        "h" => format!("{n} hours ago"),
        _ => age_words(days),
    };
    (words, Some(days))
}

fn describe(item: &Item, home: &Path, smart: bool) -> RowView {
    let mut pills = Vec::new();
    let (name, detail, type_label, chip, reveal) = match &item.what {
        What::Artifact {
            path, kind, git, ..
        } => {
            if let Some(g) = git {
                if g.dirty {
                    pills.push(("uncommitted".to_string(), Tone::Warn));
                }
                if g.worktree && g.merged {
                    pills.push(("merged worktree".to_string(), Tone::Ok));
                } else if g.worktree {
                    pills.push(("worktree".to_string(), Tone::Neutral));
                }
            }
            let project = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (
                project,
                pretty_path(path, home),
                kind.clone(),
                Chip::Mono(theme::monogram(kind), theme::kind_color(kind)),
                Some(path.clone()),
            )
        }
        What::Cache { def, paths } => {
            if let Some(cmd) = def.smart {
                if smart && has_command(cmd[0]) {
                    pills.push(("smart prune".to_string(), Tone::Accent));
                }
            }
            (
                def.name.to_string(),
                paths
                    .iter()
                    .map(|p| pretty_path(p, home))
                    .collect::<Vec<_>>()
                    .join(", "),
                "Cache".to_string(),
                Chip::Glyph(Icon::Layers, theme::CACHE_SLATE),
                paths.first().cloned(),
            )
        }
        What::Docker(o) => {
            let mut parts = o.label.split(" · ");
            let name = parts.next().unwrap_or_default().to_string();
            match o.tier {
                Tier::Unused => pills.push(("unused".to_string(), Tone::Neutral)),
                Tier::InUse => pills.push(("in use".to_string(), Tone::Neutral)),
                Tier::Volume => pills.push(("may hold data".to_string(), Tone::Warn)),
                Tier::Safe if o.kind == DockerKind::Image => {
                    pills.push(("dangling".to_string(), Tone::Neutral))
                }
                Tier::Safe => {}
            }
            let detail = parts
                .filter(|r| {
                    !r.starts_with("unused")
                        && !r.starts_with("used by")
                        && !r.starts_with("may hold")
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let type_label = match o.kind {
                DockerKind::Image => "Image",
                DockerKind::Container => "Container",
                DockerKind::Volume => "Volume",
                DockerKind::BuildCache => "Build cache",
            };
            (
                name,
                if detail.is_empty() {
                    "Docker".into()
                } else {
                    detail
                },
                type_label.to_string(),
                Chip::Glyph(Icon::Cube, theme::DOCKER_BLUE),
                None,
            )
        }
        What::Big { path, kind, .. } => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let parent = path
                .parent()
                .map(|p| pretty_path(p, home))
                .unwrap_or_default();
            let mut t = kind.to_string();
            if let Some(f) = t.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            (
                name,
                parent,
                t,
                Chip::Glyph(Icon::File, theme::FILE_TAN),
                Some(path.clone()),
            )
        }
    };
    let (age, age_days) = match &item.what {
        What::Docker(o) => docker_age(&o.age),
        What::Cache { .. } => ("—".to_string(), None),
        _ => match item.age_days() {
            Some(d) => (age_words(d), Some(d)),
            None => ("—".to_string(), None),
        },
    };
    let warn = pills.iter().any(|(_, t)| *t == Tone::Warn);
    let kind = match &item.what {
        What::Artifact { kind, .. } => kind.clone(),
        _ => type_label.clone(),
    };
    RowView {
        category: item.category(),
        haystack: format!("{name} {detail} {type_label}").to_lowercase(),
        kind,
        type_label,
        chip,
        name,
        detail,
        age,
        age_days,
        size: item.size,
        size_label: human_bytes(item.size),
        pills,
        warn,
        reveal,
    }
}

fn category_icon(c: Option<Category>) -> Icon {
    match c {
        None => Icon::Grid,
        Some(Category::Project) => Icon::Folder,
        Some(Category::Cache) => Icon::Layers,
        Some(Category::Docker) => Icon::Cube,
        Some(Category::Big) => Icon::File,
    }
}

fn pick_folder() -> Option<String> {
    #[cfg(target_os = "macos")]
    let out = Command::new("osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Choose a folder to scan\")",
        ])
        .output()
        .ok()?;
    #[cfg(not(target_os = "macos"))]
    let out = Command::new("zenity")
        .args([
            "--file-selection",
            "--directory",
            "--title=Choose a folder to scan",
        ])
        .output()
        .ok()?;
    out.status.success().then(|| {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.len() > 1 {
            s.trim_end_matches('/').to_string()
        } else {
            s
        }
    })
}

fn open_path(p: &Path) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("-t").arg(p).spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = Command::new("xdg-open").arg(p).spawn();
}

// ───────────────────────────── widgets ─────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Check {
    Off,
    On,
    Mixed,
}

fn paint_check(ui: &Ui, rect: Rect, state: Check, p: &Palette) {
    let r = Rect::from_center_size(rect.center(), Vec2::splat(15.0));
    let painter = ui.painter();
    match state {
        Check::Off => {
            painter.rect_stroke(r, 4.0, Stroke::new(1.2, p.faint), StrokeKind::Inside);
        }
        Check::On | Check::Mixed => {
            painter.rect_filled(r, 4.0, p.accent);
            if state == Check::On {
                icons::paint(painter, r.shrink(2.0), Icon::Check, p.on_accent);
            } else {
                painter.line_segment(
                    [
                        r.left_center() + Vec2::new(4.0, 0.0),
                        r.right_center() - Vec2::new(4.0, 0.0),
                    ],
                    Stroke::new(1.8, p.on_accent),
                );
            }
        }
    }
}

fn check_widget(ui: &mut Ui, state: Check, p: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    paint_check(ui, rect, state, p);
    resp
}

/// iOS-style switch.
fn toggle(ui: &mut Ui, on: &mut bool, p: &Palette) -> egui::Response {
    let size = Vec2::new(32.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { p.accent } else { p.border };
    ui.painter().rect_filled(rect, 9.0, bg);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    ui.painter()
        .circle_filled(Pos2::new(x, rect.center().y), 7.0, Color32::WHITE);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn pill(ui: &mut Ui, text: &str, tone: Tone, p: &Palette) {
    let color = match tone {
        Tone::Neutral => p.dim,
        Tone::Ok => p.ok,
        Tone::Warn => p.warn,
        Tone::Accent => p.accent,
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(10.5), color);
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, color.gamma_multiply(0.14));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn chip(ui: &mut Ui, chip: &Chip) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
    match chip {
        Chip::Mono(text, color) => {
            ui.painter()
                .rect_filled(rect, 7.0, color.gamma_multiply(0.18));
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional(11.5),
                *color,
            );
        }
        Chip::Glyph(icon, color) => {
            ui.painter()
                .rect_filled(rect, 7.0, color.gamma_multiply(0.16));
            icons::paint(ui.painter(), rect.shrink(7.0), *icon, *color);
        }
    }
}

fn section_label(ui: &mut Ui, text: &str, p: &Palette) {
    ui.label(RichText::new(text).size(10.5).color(p.faint).strong());
}

fn card<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(14))
        .show(ui, add)
        .inner
}

fn primary_button(
    ui: &mut Ui,
    text: &str,
    fill: Color32,
    fg: Color32,
    enabled: bool,
) -> egui::Response {
    let b = egui::Button::new(RichText::new(text).color(fg).strong())
        .fill(if enabled {
            fill
        } else {
            fill.gamma_multiply(0.35)
        })
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(0.0, 32.0));
    ui.add_enabled(enabled, b)
}

/// Settings row: title + description on the left, a control on the right.
fn setting_row(ui: &mut Ui, p: &Palette, title: &str, desc: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 180.0).max(200.0));
            ui.label(RichText::new(title).color(p.text));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(12.0).color(p.faint));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), add);
    });
    ui.add_space(8.0);
}

// ───────────────────────────── app ─────────────────────────────

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        let home = dirs::home_dir().unwrap_or_default();
        let cfg = Config::load(&home).unwrap_or_default();
        let opts = Options::from_config(&cfg, &home);
        let paths = if cfg.paths.is_empty() {
            vec!["~".to_string()]
        } else {
            cfg.paths.clone()
        };
        App {
            disk: disk_space(&home),
            stats: stats::load(),
            trash: cfg.trash,
            smart: cfg.smart_caches.unwrap_or(true),
            older_days: opts
                .older_than
                .map(|d| (d.as_secs() / 86_400) as u32)
                .unwrap_or(0),
            min_mb: (opts.min_size / (1024 * 1024)) as u32,
            home,
            cfg,
            paths,
            new_path: String::new(),
            include: [true, true, true, false],
            git: true,
            settings_note: None,
            items: Arc::new(Vec::new()),
            rows: Vec::new(),
            selected: Vec::new(),
            gone: Vec::new(),
            scanned: false,
            warnings: Vec::new(),
            view: View::Cleanup(None),
            kind_filter: None,
            search: String::new(),
            focus_search: false,
            sort: (SortKey::Size, true),
            collapsed: HashSet::new(),
            pending: None,
            toast: None,
            phase: Phase::Idle,
            shared: Arc::default(),
            scan_started: Instant::now(),
            snapshot: std::env::var_os("DISKDIET_SNAPSHOT").map(|p| (PathBuf::from(p), 0)),
        }
    }

    fn options(&self) -> Options {
        let mut o = Options::from_config(&self.cfg, &self.home);
        o.paths = self
            .paths
            .iter()
            .map(|p| expand_tilde(p, &self.home))
            .collect();
        o.older_than =
            (self.older_days > 0).then(|| Duration::from_secs(self.older_days as u64 * 86_400));
        o.min_size = self.min_mb as u64 * 1024 * 1024;
        [o.projects, o.caches, o.docker, o.big] = self.include;
        o.git = self.git;
        o
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        if self.phase != Phase::Idle {
            return;
        }
        self.phase = Phase::Scanning;
        self.scan_started = Instant::now();
        self.toast = None;
        self.pending = None;
        let (opts, cfg, home) = (self.options(), self.cfg.clone(), self.home.clone());
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        std::thread::spawn(move || {
            let report = |m: &str| {
                shared.lock().unwrap().message = m.to_string();
                ctx.request_repaint();
            };
            report("Scanning…");
            let scan = engine::gather(&opts, &cfg, &home, &report);
            shared.lock().unwrap().scan = Some(scan);
            ctx.request_repaint();
        });
    }

    fn start_delete(&mut self, ctx: &egui::Context, idx: Vec<usize>) {
        if idx.is_empty() || self.phase != Phase::Idle {
            return;
        }
        self.phase = Phase::Deleting;
        self.pending = None;
        let items = self.items.clone();
        let home = self.home.clone();
        let opts = DeleteOptions {
            trash: self.trash,
            smart_caches: self.smart,
        };
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        shared.lock().unwrap().progress = (0, idx.len());
        std::thread::spawn(move || {
            let before = diskdiet::util::free_space(&home);
            let refs: Vec<&Item> = idx.iter().map(|&i| &items[i]).collect();
            let outcome = engine::delete(&refs, &home, opts, &|done, total| {
                shared.lock().unwrap().progress = (done, total);
                ctx.request_repaint();
            });
            let _ = stats::record(
                outcome.freed,
                outcome.trashed,
                outcome.removed as u64,
                "gui",
            );
            shared.lock().unwrap().done = Some(Finished {
                outcome,
                removed: idx,
                before,
            });
            ctx.request_repaint();
        });
    }

    /// Pull results from worker threads.
    fn poll(&mut self) {
        let mut s = self.shared.lock().unwrap();
        if let Some(scan) = s.scan.take() {
            self.warnings = scan.warnings;
            self.rows = scan
                .items
                .iter()
                .map(|i| describe(i, &self.home, self.smart))
                .collect();
            self.selected = scan.items.iter().map(|i| i.default_selected()).collect();
            self.gone = vec![false; scan.items.len()];
            self.items = Arc::new(scan.items);
            self.scanned = true;
            self.phase = Phase::Idle;
            self.kind_filter = None;
            self.disk = disk_space(&self.home);
        }
        if let Some(done) = s.done.take() {
            let o = &done.outcome;
            for &i in &done.removed {
                // An item failed if its path or name appears in an error message.
                let r = &self.rows[i];
                let failed = o
                    .errors
                    .iter()
                    .any(|e| e.contains(&r.detail) || e.starts_with(&r.name));
                if !failed {
                    self.gone[i] = true;
                    self.selected[i] = false;
                }
            }
            self.disk = disk_space(&self.home);
            self.stats = stats::load();
            let mut body = Vec::new();
            if let (Some(b), Some((_, after))) = (done.before, self.disk) {
                body.push(format!(
                    "Free space {} → {}",
                    human_bytes(b),
                    human_bytes(after)
                ));
            }
            if o.trashed > 0 {
                body.push(format!(
                    "{} moved to the Trash; empty it to get the space back",
                    human_bytes(o.trashed)
                ));
            }
            let freed = human_bytes(o.freed + o.trashed);
            self.toast = Some(Toast {
                ok: o.errors.is_empty(),
                title: if o.errors.is_empty() {
                    format!("Freed {freed}")
                } else {
                    format!(
                        "Freed {freed} · {} item(s) could not be removed",
                        o.errors.len()
                    )
                },
                body: body.join(" · "),
                errors: o.errors.clone(),
            });
            self.phase = Phase::Idle;
        }
    }

    fn view_category(&self) -> Option<Option<Category>> {
        match self.view {
            View::Cleanup(c) => Some(c),
            _ => None,
        }
    }

    fn in_view(&self, i: usize, cat: Option<Category>) -> bool {
        !self.gone[i] && cat.is_none_or(|c| self.rows[i].category == c)
    }

    /// Indices visible in the current list, sorted.
    fn visible(&self) -> Vec<usize> {
        let Some(cat) = self.view_category() else {
            return Vec::new();
        };
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.in_view(i, cat))
            .filter(|&i| {
                self.kind_filter
                    .as_ref()
                    .is_none_or(|k| &self.rows[i].kind == k)
            })
            .filter(|&i| q.is_empty() || self.rows[i].haystack.contains(&q))
            .collect();
        let (key, desc) = self.sort;
        v.sort_by(|&a, &b| {
            let (ra, rb) = (&self.rows[a], &self.rows[b]);
            let ord = match key {
                SortKey::Size => ra.size.cmp(&rb.size),
                SortKey::Name => ra.name.to_lowercase().cmp(&rb.name.to_lowercase()),
                SortKey::Age => ra.age_days.unwrap_or(0).cmp(&rb.age_days.unwrap_or(0)),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
        v
    }

    fn lines(&self, visible: &[usize]) -> Vec<Line> {
        let grouped = self.view == View::Cleanup(None)
            && self.search.trim().is_empty()
            && self.kind_filter.is_none();
        if !grouped {
            return visible.iter().map(|&i| Line::Item(i)).collect();
        }
        let mut out = Vec::new();
        for c in CATEGORIES {
            let members: Vec<usize> = visible
                .iter()
                .copied()
                .filter(|&i| self.rows[i].category == c)
                .collect();
            if members.is_empty() {
                continue;
            }
            out.push(Line::Group(c));
            if !self.collapsed.contains(&c) {
                out.extend(members.into_iter().map(Line::Item));
            }
        }
        out
    }

    fn selected_all(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&i| !self.gone[i] && self.selected[i])
            .collect()
    }

    fn totals(&self, cat: Option<Category>) -> (usize, u64) {
        (0..self.rows.len())
            .filter(|&i| self.in_view(i, cat))
            .fold((0, 0), |(n, b), i| (n + 1, b + self.rows[i].size))
    }

    fn check_state(&self, idx: impl Iterator<Item = usize>) -> Check {
        let (mut on, mut n) = (0, 0);
        for i in idx {
            n += 1;
            if self.selected[i] {
                on += 1;
            }
        }
        match (on, n) {
            (0, _) => Check::Off,
            (a, b) if a == b => Check::On,
            _ => Check::Mixed,
        }
    }

    fn set_all(&mut self, idx: &[usize], on: bool) {
        for &i in idx {
            self.selected[i] = on;
        }
    }

    fn snapshot_tick(&mut self, ctx: &egui::Context) {
        if !self.scanned || self.phase != Phase::Idle {
            return;
        }
        let Some((path, frames)) = self.snapshot.as_mut() else {
            return;
        };
        *frames += 1;
        let (path, frames) = (path.clone(), *frames);
        ctx.request_repaint();
        if frames == 1 {
            match std::env::var("DISKDIET_SNAPSHOT_VIEW").as_deref() {
                Ok("settings") => self.view = View::Settings,
                Ok("stats") => self.view = View::Stats,
                Ok("projects") => self.view = View::Cleanup(Some(Category::Project)),
                _ => {}
            }
            if std::env::var("DISKDIET_SNAPSHOT_THEME").as_deref() == Ok("light") {
                ctx.set_theme(egui::Theme::Light);
            }
        }
        if frames == 6 {
            let c = ctx.clone();
            ctx.request_screenshot(move |img| {
                let _ = write_bmp(&path, &img);
                c.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (find, rescan, all, del, esc, nums) = ctx.input_mut(|i| {
            let nums: Vec<bool> = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
            ]
            .iter()
            .map(|k| i.consume_key(Modifiers::COMMAND, *k))
            .collect();
            (
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::COMMAND, Key::R),
                !typing && i.consume_key(Modifiers::COMMAND, Key::A),
                i.consume_key(Modifiers::COMMAND, Key::Backspace),
                i.key_pressed(Key::Escape),
                nums,
            )
        });
        if find {
            if self.view_category().is_none() {
                self.view = View::Cleanup(None);
            }
            self.focus_search = true;
        }
        if rescan {
            self.start_scan(ctx);
        }
        if all {
            let v = self.visible();
            let on = self.check_state(v.iter().copied()) != Check::On;
            self.set_all(&v, on);
        }
        if del && self.phase == Phase::Idle && !self.selected_all().is_empty() {
            self.pending = Some(self.selected_all());
        }
        if esc {
            if self.pending.is_some() {
                self.pending = None;
            } else if !self.search.is_empty() {
                self.search.clear();
            }
        }
        let views = [
            View::Cleanup(None),
            View::Cleanup(Some(Category::Project)),
            View::Cleanup(Some(Category::Cache)),
            View::Cleanup(Some(Category::Docker)),
            View::Cleanup(Some(Category::Big)),
            View::Stats,
            View::Settings,
        ];
        for (pressed, v) in nums.iter().zip(views) {
            if *pressed {
                self.view = v;
                self.kind_filter = None;
            }
        }
    }
}

// ───────────────────────────── UI ─────────────────────────────

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        self.snapshot_tick(&ctx);
        self.shortcuts(&ctx);
        let p = theme::current(&ctx);
        if self.phase != Phase::Idle {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        self.sidebar(ui, &p);
        if self.view_category().is_some() && self.scanned {
            self.action_bar(ui, &p);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(22, 18)),
            )
            .show(ui, |ui| match self.view {
                View::Cleanup(cat) => {
                    if self.scanned {
                        self.cleanup(ui, cat, &p);
                    } else {
                        self.welcome(ui, &p);
                    }
                }
                View::Stats => self.stats_view(ui, &p),
                View::Settings => self.settings_view(ui, &p),
            });
        self.confirm_dialog(&ctx, &p);
    }
}

impl App {
    fn sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(232.0)
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .stroke(Stroke::new(1.0, p.border))
                    .inner_margin(egui::Margin::symmetric(12, 14)),
            )
            .show(ui, |ui| {
                // Brand
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    icons::mark(ui.painter(), r, p.accent, p.panel);
                    ui.label(RichText::new("diskdiet").size(16.0).strong().color(p.text));
                    let g = ui.painter().layout_no_wrap(
                        "BETA".into(),
                        FontId::proportional(9.5),
                        p.warn,
                    );
                    let (r, _) =
                        ui.allocate_exact_size(g.size() + Vec2::new(10.0, 4.0), Sense::hover());
                    ui.painter().rect_stroke(
                        r,
                        4.0,
                        Stroke::new(1.0, p.warn.gamma_multiply(0.7)),
                        StrokeKind::Inside,
                    );
                    ui.painter().galley(r.center() - g.size() / 2.0, g, p.warn);
                });
                ui.add_space(18.0);

                section_label(ui, "CLEAN UP", p);
                ui.add_space(2.0);
                let mut nav = vec![(View::Cleanup(None), "Everything", Icon::Grid)];
                for c in CATEGORIES {
                    nav.push((View::Cleanup(Some(c)), c.label(), category_icon(Some(c))));
                }
                for (view, label, icon) in nav {
                    let count = match view {
                        View::Cleanup(c) if self.scanned => Some(self.totals(c).0),
                        _ => None,
                    };
                    self.nav_item(ui, p, view, label, icon, count);
                }
                ui.add_space(14.0);
                section_label(ui, "APP", p);
                ui.add_space(2.0);
                self.nav_item(ui, p, View::Stats, "Stats", Icon::Chart, None);
                self.nav_item(ui, p, View::Settings, "Settings", Icon::Gear, None);

                // Pin the storage card to the bottom, laid out top-down.
                let lines = 2
                    + usize::from(self.stats.total_freed > 0)
                    + usize::from(!self.trash && !self.selected_all().is_empty());
                let card_h = 66.0 + 17.0 * lines as f32;
                let gap = ui.available_height() - card_h;
                if gap > 0.0 {
                    ui.add_space(gap);
                }
                self.storage_card(ui, p);
            });
    }

    fn nav_item(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        view: View,
        label: &str,
        icon: Icon,
        count: Option<usize>,
    ) {
        let active = self.view == view;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
        if active {
            ui.painter()
                .rect_filled(rect, 7.0, p.accent.gamma_multiply(0.14));
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 7.0, p.hover);
        }
        let fg = if active { p.text } else { p.dim };
        let ic =
            Rect::from_center_size(rect.left_center() + Vec2::new(17.0, 0.0), Vec2::splat(15.0));
        icons::paint(
            ui.painter(),
            ic,
            icon,
            if active { p.accent } else { p.faint },
        );
        ui.painter().text(
            rect.left_center() + Vec2::new(34.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.5),
            fg,
        );
        if let Some(n) = count {
            let g = ui
                .painter()
                .layout_no_wrap(n.to_string(), FontId::proportional(11.0), p.dim);
            let r = Rect::from_center_size(
                rect.right_center() - Vec2::new(13.0 + g.size().x / 2.0, 0.0),
                g.size() + Vec2::new(10.0, 3.0),
            );
            ui.painter()
                .rect_filled(r, r.height() / 2.0, p.border.gamma_multiply(0.7));
            ui.painter().galley(r.center() - g.size() / 2.0, g, p.dim);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.view = view;
            self.kind_filter = None;
        }
    }

    fn storage_card(&self, ui: &mut Ui, p: &Palette) {
        let Some((total, free)) = self.disk else {
            return;
        };
        let selected: u64 = if self.trash {
            0
        } else {
            self.selected_all().iter().map(|&i| self.rows[i].size).sum()
        };
        let ratio_free = free as f32 / total.max(1) as f32;
        let (status, color) = if ratio_free < 0.05 {
            ("Almost full", p.danger)
        } else if ratio_free < 0.15 {
            ("Running low", p.warn)
        } else {
            ("Healthy", p.ok)
        };
        card(ui, p, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Storage").strong().color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(status).size(11.0).color(color));
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, color);
                });
            });
            ui.add_space(2.0);
            // Meter: used | selected-for-cleanup | free
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, p.border);
            let used = total.saturating_sub(free);
            let w = |b: u64| rect.width() * (b as f32 / total.max(1) as f32);
            let used_r = Rect::from_min_size(rect.min, Vec2::new(w(used), rect.height()));
            painter.rect_filled(used_r, 4.0, p.dim.gamma_multiply(0.8));
            if selected > 0 {
                let sel_w = w(selected.min(used)).max(3.0);
                let sel_r = Rect::from_min_max(
                    Pos2::new(used_r.right() - sel_w, rect.top()),
                    Pos2::new(used_r.right(), rect.bottom()),
                );
                painter.rect_filled(sel_r, 2.0, p.accent);
            }
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!(
                    "{} free of {}",
                    human_bytes(free),
                    human_bytes(total)
                ))
                .size(11.5)
                .color(p.dim),
            );
            if selected > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} free after cleanup",
                        human_bytes(free + selected)
                    ))
                    .size(11.5)
                    .color(p.accent),
                );
            }
            if self.stats.total_freed > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} freed with diskdiet so far",
                        human_bytes(self.stats.total_freed)
                    ))
                    .size(11.0)
                    .color(p.faint),
                );
            }
        });
    }

    fn welcome(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        ui.add_space((ui.available_height() * 0.22).max(20.0));
        ui.vertical_centered(|ui| {
            if self.phase == Phase::Scanning {
                ui.add(egui::Spinner::new().size(28.0).color(p.accent));
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Scanning your storage…")
                        .heading()
                        .color(p.text),
                );
                ui.add_space(4.0);
                let msg = self.shared.lock().unwrap().message.clone();
                ui.label(RichText::new(msg).color(p.dim));
                ui.label(
                    RichText::new(format!(
                        "{}s · big folders like your home can take a minute",
                        self.scan_started.elapsed().as_secs()
                    ))
                    .size(11.5)
                    .color(p.faint),
                );
                return;
            }
            let (r, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
            icons::mark(ui.painter(), r, p.accent, p.bg);
            ui.add_space(14.0);
            ui.label(
                RichText::new("Find what's eating your storage")
                    .size(24.0)
                    .strong()
                    .color(p.text),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "node_modules, build folders, package caches, Docker leftovers and big forgotten files.\nNothing is deleted until you choose.",
                )
                .color(p.dim),
            );
            ui.add_space(16.0);
            ui.label(
                RichText::new(format!("Folders: {}", self.paths.join(", ")))
                    .size(12.5)
                    .color(p.faint),
            );
            if ui.link(RichText::new("Change folders").size(12.5)).clicked() {
                self.view = View::Settings;
            }
            ui.add_space(14.0);
            if primary_button(ui, "     Scan     ", p.accent, p.on_accent, true)
                .on_hover_text("⌘R")
                .clicked()
            {
                self.start_scan(&ctx);
            }
            ui.add_space(6.0);
            ui.label(RichText::new("⌘R").size(11.0).color(p.faint));
        });
    }

    fn top_bar(&mut self, ui: &mut Ui, cat: Option<Category>, p: &Palette) {
        let ctx = ui.ctx().clone();
        let (n, bytes) = self.totals(cat);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let title = cat.map(|c| c.label()).unwrap_or("Everything");
                ui.label(RichText::new(title).size(21.0).strong().color(p.text));
                ui.horizontal(|ui| {
                    if self.phase == Phase::Scanning {
                        ui.add(egui::Spinner::new().size(12.0).color(p.accent));
                        let msg = self.shared.lock().unwrap().message.clone();
                        ui.label(RichText::new(msg).size(12.0).color(p.dim));
                    } else {
                        ui.label(
                            RichText::new(format!(
                                "{n} items · {} reclaimable",
                                human_bytes(bytes)
                            ))
                            .size(12.0)
                            .color(p.dim),
                        );
                    }
                });
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = self.phase == Phase::Idle;
                let rescan = ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(RichText::new("Rescan").color(p.text))
                            .min_size(Vec2::new(0.0, 30.0)),
                    )
                    .on_hover_text("⌘R");
                if rescan.clicked() {
                    self.start_scan(&ctx);
                }
                // Search field with icon and shortcut hint.
                egui::Frame::new()
                    .fill(p.input)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(250.0, 22.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                icons::icon(ui, Icon::Search, 14.0, p.faint);
                                let te = egui::TextEdit::singleline(&mut self.search)
                                    .frame(egui::Frame::NONE)
                                    .hint_text("Search name or path")
                                    .desired_width(196.0);
                                let resp = ui.add(te);
                                if self.focus_search {
                                    resp.request_focus();
                                    self.focus_search = false;
                                }
                                ui.label(RichText::new("⌘F").size(11.0).color(p.faint));
                            },
                        );
                    });
            });
        });
    }

    fn kind_pills(&mut self, ui: &mut Ui, cat: Option<Category>, p: &Palette) {
        let mut kinds: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for i in 0..self.rows.len() {
            if self.in_view(i, cat) {
                let e = kinds.entry(self.rows[i].kind.clone()).or_default();
                e.0 += 1;
                e.1 += self.rows[i].size;
            }
        }
        let mut kinds: Vec<(String, (usize, u64))> = kinds.into_iter().collect();
        kinds.sort_by_key(|(_, (_, b))| std::cmp::Reverse(*b));
        kinds.truncate(7);
        let total = self.totals(cat).0;
        let mut clicked: Option<Option<String>> = None;
        let visible = self.visible();
        let mut select: Option<bool> = None;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let pill_btn = |ui: &mut Ui, label: &str, n: usize, active: bool| -> bool {
                let g = ui.painter().layout_no_wrap(
                    format!("{label}  {n}"),
                    FontId::proportional(12.0),
                    if active { p.text } else { p.dim },
                );
                let (r, resp) =
                    ui.allocate_exact_size(g.size() + Vec2::new(20.0, 10.0), Sense::click());
                let (fill, stroke) = if active {
                    (p.accent.gamma_multiply(0.14), p.accent.gamma_multiply(0.7))
                } else if resp.hovered() {
                    (p.hover, p.border)
                } else {
                    (Color32::TRANSPARENT, p.border)
                };
                ui.painter().rect_filled(r, r.height() / 2.0, fill);
                ui.painter().rect_stroke(
                    r,
                    r.height() / 2.0,
                    Stroke::new(1.0, stroke),
                    StrokeKind::Inside,
                );
                ui.painter().galley(r.center() - g.size() / 2.0, g, p.text);
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                resp.clicked()
            };
            if pill_btn(ui, "All", total, self.kind_filter.is_none()) {
                clicked = Some(None);
            }
            for (k, (n, _)) in &kinds {
                if pill_btn(ui, k, *n, self.kind_filter.as_deref() == Some(k.as_str())) {
                    clicked = Some(Some(k.clone()));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.link(RichText::new("Select none").size(12.0)).clicked() {
                    select = Some(false);
                }
                ui.label(RichText::new("·").color(p.faint));
                if ui
                    .link(RichText::new("Select all").size(12.0))
                    .on_hover_text("⌘A")
                    .clicked()
                {
                    select = Some(true);
                }
            });
        });
        if let Some(c) = clicked {
            self.kind_filter = c;
        }
        if let Some(on) = select {
            self.set_all(&visible, on);
        }
    }

    fn toast_banner(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(t) = &self.toast else { return };
        let color = if t.ok { p.ok } else { p.warn };
        let mut close = false;
        egui::Frame::new()
            .fill(color.gamma_multiply(0.10))
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    icons::icon(
                        ui,
                        if t.ok { Icon::Check } else { Icon::Warning },
                        18.0,
                        color,
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&t.title).strong().color(p.text));
                        if !t.body.is_empty() {
                            ui.label(RichText::new(&t.body).size(12.0).color(p.dim));
                        }
                        if !t.errors.is_empty() {
                            egui::CollapsingHeader::new(RichText::new("Show errors").size(12.0))
                                .id_salt("toast-errors")
                                .show(ui, |ui| {
                                    for e in &t.errors {
                                        ui.label(RichText::new(e).size(11.5).color(p.dim));
                                    }
                                });
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        if icons::icon_button(ui, Icon::Close, p.faint, p.text, "Dismiss").clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if close {
            self.toast = None;
        }
        ui.add_space(10.0);
    }

    fn cleanup(&mut self, ui: &mut Ui, cat: Option<Category>, p: &Palette) {
        self.top_bar(ui, cat, p);
        ui.add_space(12.0);
        self.toast_banner(ui, p);
        for w in &self.warnings {
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Warning, 14.0, p.warn);
                ui.label(RichText::new(w).size(12.0).color(p.warn));
            });
        }
        self.kind_pills(ui, cat, p);
        ui.add_space(10.0);

        let visible = self.visible();
        if visible.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                icons::icon(ui, category_icon(cat), 28.0, p.faint);
                ui.add_space(8.0);
                let msg = if !self.search.is_empty() || self.kind_filter.is_some() {
                    "Nothing matches your filter."
                } else if cat == Some(Category::Big) && !self.include[3] {
                    "Big files aren't scanned yet. Turn them on in Settings, then rescan."
                } else {
                    "Nothing to clean here. Already lean."
                };
                ui.label(RichText::new(msg).color(p.dim));
            });
            return;
        }
        self.table(ui, &visible, p);
    }

    fn table(&mut self, ui: &mut Ui, visible: &[usize], p: &Palette) {
        let lines = self.lines(visible);
        let max = visible
            .iter()
            .map(|&i| self.rows[i].size)
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let busy = self.phase != Phase::Idle;
        let master = self.check_state(visible.iter().copied());
        let mut toggle_master = false;
        let mut toggles: Vec<usize> = Vec::new();
        let mut group_set: Option<(Category, bool)> = None;
        let mut collapse: Option<Category> = None;
        let mut delete_one: Option<usize> = None;
        let mut sort_click: Option<SortKey> = None;
        let sort = self.sort;
        let rows = &self.rows;
        let selected = &self.selected;
        let collapsed = &self.collapsed;

        let group_state = |c: Category| -> (Check, usize, u64) {
            let mut on = 0;
            let mut n = 0;
            let mut bytes = 0;
            for &i in visible.iter().filter(|&&i| rows[i].category == c) {
                n += 1;
                bytes += rows[i].size;
                if selected[i] {
                    on += 1;
                }
            }
            let st = match on {
                0 => Check::Off,
                x if x == n => Check::On,
                _ => Check::Mixed,
            };
            (st, n, bytes)
        };

        egui::Frame::new()
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(4, 2))
            .show(ui, |ui| {
                let heights = lines.iter().map(|l| match l {
                    Line::Group(_) => 36.0,
                    Line::Item(_) => 50.0,
                });
                TableBuilder::new(ui)
                    .id_salt("results")
                    .sense(Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(34.0))
                    .column(Column::remainder().clip(true).at_least(220.0))
                    .column(Column::exact(104.0))
                    .column(Column::exact(108.0))
                    .column(Column::exact(118.0))
                    .column(Column::exact(66.0))
                    .header(30.0, |mut h| {
                        h.col(|ui| {
                            if check_widget(ui, master, p).clicked() && !busy {
                                toggle_master = true;
                            }
                        });
                        let mut head =
                            |ui: &mut Ui, label: &str, key: Option<SortKey>, right: bool| {
                                let arrow = match key {
                                    Some(k) if k == sort.0 => {
                                        if sort.1 {
                                            " ↓"
                                        } else {
                                            " ↑"
                                        }
                                    }
                                    _ => "",
                                };
                                let text = RichText::new(format!("{label}{arrow}"))
                                    .size(10.5)
                                    .strong()
                                    .color(p.faint);
                                let layout = if right {
                                    Layout::right_to_left(Align::Center)
                                } else {
                                    Layout::left_to_right(Align::Center)
                                };
                                ui.with_layout(layout, |ui| {
                                    let sense = if key.is_some() {
                                        Sense::click()
                                    } else {
                                        Sense::hover()
                                    };
                                    let r = ui.add(egui::Label::new(text).sense(sense));
                                    if let Some(k) = key {
                                        if r.hovered() {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }
                                        if r.clicked() {
                                            sort_click = Some(k);
                                        }
                                    }
                                });
                            };
                        h.col(|ui| head(ui, "NAME", Some(SortKey::Name), false));
                        h.col(|ui| head(ui, "TYPE", None, false));
                        h.col(|ui| head(ui, "LAST TOUCHED", Some(SortKey::Age), false));
                        h.col(|ui| head(ui, "SIZE", Some(SortKey::Size), true));
                        h.col(|ui| head(ui, "", None, false));
                    })
                    .body(|body| {
                        body.heterogeneous_rows(heights, |mut row| match lines[row.index()] {
                            Line::Group(c) => {
                                let (st, n, bytes) = group_state(c);
                                let is_collapsed = collapsed.contains(&c);
                                row.col(|ui| {
                                    if check_widget(ui, st, p).clicked() && !busy {
                                        group_set = Some((c, st != Check::On));
                                    }
                                });
                                row.col(|ui| {
                                    let chev = if is_collapsed {
                                        Icon::ChevronRight
                                    } else {
                                        Icon::ChevronDown
                                    };
                                    icons::icon(ui, chev, 14.0, p.dim);
                                    icons::icon(ui, category_icon(Some(c)), 15.0, p.dim);
                                    ui.label(RichText::new(c.label()).strong().color(p.text));
                                    ui.label(
                                        RichText::new(format!("{n} items"))
                                            .size(12.0)
                                            .color(p.faint),
                                    );
                                });
                                row.col(|_| {});
                                row.col(|_| {});
                                row.col(|ui| {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(human_bytes(bytes))
                                                .monospace()
                                                .strong()
                                                .color(p.text),
                                        );
                                    });
                                });
                                row.col(|_| {});
                                if row.response().clicked() && group_set.is_none() {
                                    collapse = Some(c);
                                }
                            }
                            Line::Item(i) => {
                                let r = &rows[i];
                                let sel = selected[i];
                                row.set_selected(sel);
                                let mut action = false;
                                row.col(|ui| {
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                                    paint_check(
                                        ui,
                                        rect,
                                        if sel { Check::On } else { Check::Off },
                                        p,
                                    );
                                });
                                row.col(|ui| {
                                    chip(ui, &r.chip);
                                    ui.add_space(2.0);
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&r.name).color(p.text).strong(),
                                                )
                                                .truncate(),
                                            );
                                            for (text, tone) in &r.pills {
                                                pill(ui, text, *tone, p);
                                            }
                                        });
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&r.detail).size(11.5).color(p.faint),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&r.detail);
                                    });
                                });
                                row.col(|ui| {
                                    ui.label(RichText::new(&r.type_label).size(12.5).color(p.dim));
                                });
                                row.col(|ui| {
                                    ui.label(RichText::new(&r.age).size(12.5).color(p.dim));
                                });
                                row.col(|ui| {
                                    let rect = ui.max_rect();
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let c = if r.size >= 1 << 30 { p.text } else { p.dim };
                                        ui.label(RichText::new(&r.size_label).monospace().color(c));
                                    });
                                    let w = (r.size as f32 / max).sqrt() * (rect.width() - 10.0);
                                    let y = rect.bottom() - 8.0;
                                    ui.painter().line_segment(
                                        [
                                            Pos2::new(rect.right() - w.max(3.0), y),
                                            Pos2::new(rect.right(), y),
                                        ],
                                        Stroke::new(2.5, r.chip.color().gamma_multiply(0.55)),
                                    );
                                });
                                row.col(|ui| {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if !busy
                                            && icons::icon_button(
                                                ui,
                                                Icon::Trash,
                                                p.faint,
                                                p.danger,
                                                "Clean up just this",
                                            )
                                            .clicked()
                                        {
                                            delete_one = Some(i);
                                            action = true;
                                        }
                                        if let Some(path) = &r.reveal {
                                            if icons::icon_button(
                                                ui,
                                                Icon::Reveal,
                                                p.faint,
                                                p.text,
                                                "Show in Finder",
                                            )
                                            .clicked()
                                            {
                                                engine::reveal(path);
                                                action = true;
                                            }
                                        }
                                    });
                                });
                                if !action && !busy && row.response().clicked() {
                                    toggles.push(i);
                                }
                            }
                        });
                    });
            });

        if toggle_master {
            self.set_all(visible, master != Check::On);
        }
        for i in toggles {
            self.selected[i] = !self.selected[i];
        }
        if let Some((c, on)) = group_set {
            let members: Vec<usize> = visible
                .iter()
                .copied()
                .filter(|&i| self.rows[i].category == c)
                .collect();
            self.set_all(&members, on);
        }
        if let Some(c) = collapse {
            if !self.collapsed.remove(&c) {
                self.collapsed.insert(c);
            }
        }
        if let Some(i) = delete_one {
            self.pending = Some(vec![i]);
        }
        if let Some(k) = sort_click {
            self.sort = if self.sort.0 == k {
                (k, !self.sort.1)
            } else {
                (k, k != SortKey::Name)
            };
        }
    }

    fn action_bar(&mut self, ui: &mut Ui, p: &Palette) {
        egui::Panel::bottom("actions")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .stroke(Stroke::new(1.0, p.border))
                    .inner_margin(egui::Margin::symmetric(22, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let sel = self.selected_all();
                    let bytes: u64 = sel.iter().map(|&i| self.rows[i].size).sum();
                    if self.phase == Phase::Deleting {
                        let (d, t) = self.shared.lock().unwrap().progress;
                        ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                        ui.label(RichText::new(format!("Cleaning up… {d} of {t}")).color(p.text));
                        ui.add(
                            egui::ProgressBar::new(d as f32 / t.max(1) as f32)
                                .desired_width(200.0)
                                .fill(p.accent),
                        );
                    } else if sel.is_empty() {
                        ui.label(
                            RichText::new("Select items to clean up. Click a row to toggle it.")
                                .color(p.dim),
                        );
                    } else {
                        ui.label(
                            RichText::new(format!("{} selected", sel.len()))
                                .strong()
                                .color(p.text),
                        );
                        ui.label(RichText::new("·").color(p.faint));
                        ui.label(RichText::new(human_bytes(bytes)).monospace().color(p.text));
                        let warn = sel.iter().filter(|&&i| self.rows[i].warn).count();
                        if warn > 0 {
                            ui.add_space(6.0);
                            icons::icon(ui, Icon::Warning, 14.0, p.warn);
                            ui.label(
                                RichText::new(format!("{warn} need a second look"))
                                    .size(12.0)
                                    .color(p.warn),
                            );
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let idle = self.phase == Phase::Idle;
                        let label = if sel.is_empty() {
                            "Clean up".to_string()
                        } else if self.trash {
                            format!("Move {} to Trash", human_bytes(bytes))
                        } else {
                            format!("Clean up {}", human_bytes(bytes))
                        };
                        if primary_button(
                            ui,
                            &format!("  {label}  "),
                            p.danger,
                            Color32::WHITE,
                            idle && !sel.is_empty(),
                        )
                        .on_hover_text("⌘⌫")
                        .clicked()
                        {
                            self.pending = Some(sel.clone());
                        }
                        if self.trash {
                            ui.label(RichText::new("Trash mode").size(11.5).color(p.faint));
                        }
                    });
                });
            });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(idx) = self.pending.clone() else {
            return;
        };
        let bytes: u64 = idx.iter().map(|&i| self.rows[i].size).sum();
        let mut by_cat: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
        for &i in &idx {
            let e = by_cat.entry(self.rows[i].category.label()).or_default();
            e.0 += 1;
            e.1 += self.rows[i].size;
        }
        let warn = idx.iter().filter(|&&i| self.rows[i].warn).count();
        let docker = idx
            .iter()
            .filter(|&&i| self.rows[i].category == Category::Docker)
            .count();
        let verb = if self.trash {
            "Move to Trash"
        } else {
            "Clean up"
        };
        let title = if idx.len() == 1 {
            format!("{verb} {}?", self.rows[idx[0]].name)
        } else {
            format!("{verb} {} items?", idx.len())
        };
        let trash = self.trash;

        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                ui.painter()
                    .rect_filled(r, 9.0, p.danger.gamma_multiply(0.14));
                icons::paint(ui.painter(), r.shrink(9.0), Icon::Trash, p.danger);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&title).size(16.0).strong().color(p.text));
                    ui.label(
                        RichText::new(format!("About {} of storage", human_bytes(bytes)))
                            .color(p.dim),
                    );
                });
            });
            ui.add_space(10.0);
            card(ui, p, |ui| {
                ui.set_width(ui.available_width());
                for (label, (n, b)) in &by_cat {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(*label).color(p.text));
                        ui.label(RichText::new(format!("{n}")).color(p.faint));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(human_bytes(*b)).monospace().color(p.dim));
                        });
                    });
                }
            });
            ui.add_space(8.0);
            let note = |ui: &mut Ui, icon: Icon, warn: bool, text: &str| {
                ui.horizontal(|ui| {
                    icons::icon(ui, icon, 14.0, if warn { p.warn } else { p.faint });
                    ui.label(RichText::new(text).size(12.5).color(if warn {
                        p.warn
                    } else {
                        p.dim
                    }));
                });
            };
            if trash {
                note(
                    ui,
                    Icon::Check,
                    false,
                    "Items go to the Trash. Space comes back after you empty it.",
                );
            } else {
                note(
                    ui,
                    Icon::Check,
                    false,
                    "Can't be undone. Build folders and caches regenerate when needed.",
                );
            }
            if docker > 0 {
                note(
                    ui,
                    Icon::Cube,
                    false,
                    &format!("{docker} Docker item(s) are always removed permanently."),
                );
            }
            if warn > 0 {
                note(
                    ui,
                    Icon::Warning,
                    true,
                    &format!("{warn} item(s) have uncommitted changes or volume data."),
                );
            }
            ui.add_space(14.0);
            let mut go = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    go = primary_button(ui, &format!("  {verb}  "), p.danger, Color32::WHITE, true)
                        .clicked();
                    cancel = ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(0.0, 32.0)))
                        .clicked();
                });
            });
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            (go || enter, cancel)
        });
        let (go, cancel) = modal.inner;
        if go {
            self.start_delete(ctx, idx);
        } else if cancel || modal.should_close() {
            self.pending = None;
        }
    }

    fn stats_view(&mut self, ui: &mut Ui, p: &Palette) {
        let s = &self.stats;
        ui.label(RichText::new("Stats").size(21.0).strong().color(p.text));
        ui.label(
            RichText::new("Everything diskdiet has cleaned on this computer: CLI, app and background service.")
                .size(12.0)
                .color(p.dim),
        );
        ui.add_space(16.0);
        if s.runs == 0 {
            card(ui, p, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("No cleanups yet").strong().color(p.text));
                ui.label(
                    RichText::new("Scan, clean something up, and your totals show up here.")
                        .color(p.dim),
                );
            });
            return;
        }
        ui.columns(3, |cols| {
            let tiles = [
                (
                    "Freed in total",
                    human_bytes(s.total_freed + s.total_trashed),
                    true,
                ),
                ("Items removed", s.items_removed.to_string(), false),
                ("Cleanups", s.runs.to_string(), false),
            ];
            for (ui, (label, value, accent)) in cols.iter_mut().zip(tiles) {
                card(ui, p, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(label).size(11.5).color(p.faint));
                    ui.label(RichText::new(value).size(26.0).strong().color(if accent {
                        p.accent
                    } else {
                        p.text
                    }));
                });
            }
        });
        ui.add_space(16.0);
        card(ui, p, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Recent cleanups").strong().color(p.text));
            ui.add_space(8.0);
            let n_runs = s.history.len().min(24);
            let runs = &s.history[s.history.len() - n_runs..];
            let max = runs
                .iter()
                .map(|r| r.freed + r.trashed)
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), Sense::hover());
            let slot = rect.width() / 24.0;
            for (k, r) in runs.iter().enumerate() {
                let h = ((r.freed + r.trashed) as f32 / max) * (rect.height() - 10.0);
                let x = rect.left() + slot * k as f32 + slot * 0.2;
                let bar = Rect::from_min_max(
                    Pos2::new(x, rect.bottom() - h.max(2.0)),
                    Pos2::new(x + slot * 0.6, rect.bottom()),
                );
                let resp = ui.interact(
                    bar.expand(2.0),
                    ui.scope_id().with(("bar", k)),
                    Sense::hover(),
                );
                let c = if resp.hovered() {
                    p.accent
                } else {
                    p.accent.gamma_multiply(0.65)
                };
                ui.painter().rect_filled(bar, 3.0, c);
                resp.on_hover_text(format!(
                    "{} · {} item(s) · via {}",
                    human_bytes(r.freed + r.trashed),
                    r.items,
                    r.source
                ));
            }
            ui.painter().line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                Stroke::new(1.0, p.border),
            );
            ui.add_space(10.0);
            let now = now_secs();
            for r in s.history.iter().rev().take(8) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(age_words(now.saturating_sub(r.at) / 86_400)).color(p.dim),
                    );
                    ui.label(
                        RichText::new(format!("via {}", r.source))
                            .size(12.0)
                            .color(p.faint),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(human_bytes(r.freed + r.trashed))
                                .monospace()
                                .color(p.text),
                        );
                        ui.label(
                            RichText::new(format!("{} items", r.items))
                                .size(12.0)
                                .color(p.faint),
                        );
                    });
                });
            }
        });
    }

    fn settings_view(&mut self, ui: &mut Ui, p: &Palette) {
        let home = self.home.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new("Settings").size(21.0).strong().color(p.text));
            ui.label(
                RichText::new("Changes apply to the next scan. Save them to make them the default.")
                    .size(12.0)
                    .color(p.dim),
            );
            ui.add_space(16.0);

            // Folders
            card(ui, p, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Folders to scan for projects")
                        .strong()
                        .color(p.text),
                );
                ui.add_space(6.0);
                let mut remove = None;
                for (k, path) in self.paths.iter().enumerate() {
                    ui.horizontal(|ui| {
                        icons::icon(ui, Icon::Folder, 15.0, p.faint);
                        ui.label(RichText::new(path).color(p.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::icon_button(ui, Icon::Close, p.faint, p.danger, "Remove")
                                .clicked()
                            {
                                remove = Some(k);
                            }
                        });
                    });
                }
                if let Some(k) = remove {
                    self.paths.remove(k);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Choose folder…").clicked() {
                        if let Some(f) = pick_folder() {
                            let f = pretty_path(Path::new(&f), &home);
                            if !self.paths.contains(&f) {
                                self.paths.push(f);
                            }
                        }
                    }
                    ui.label(RichText::new("or").color(p.faint));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.new_path)
                            .hint_text("~/code")
                            .desired_width(200.0),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (ui.button("Add").clicked() || enter) && !self.new_path.trim().is_empty() {
                        let f = self.new_path.trim().to_string();
                        if !self.paths.contains(&f) {
                            self.paths.push(f);
                        }
                        self.new_path.clear();
                    }
                });
            });
            ui.add_space(12.0);

            // What to look for
            card(ui, p, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("What to look for").strong().color(p.text));
                ui.add_space(8.0);
                let descs = [
                    "node_modules, target, build, .venv and friends, next to their project files",
                    "npm, pnpm, Yarn, Cargo, Go, Xcode, Homebrew, Playwright and more",
                    "Images, stopped containers, unused volumes, build cache",
                    "Installers, archives, videos and backups in Downloads, Desktop, Documents, Movies",
                ];
                for (k, c) in CATEGORIES.iter().enumerate() {
                    let mut v = self.include[k];
                    setting_row(ui, p, c.label(), descs[k], |ui| {
                        toggle(ui, &mut v, p);
                    });
                    self.include[k] = v;
                }
                let mut days = self.older_days;
                setting_row(
                    ui,
                    p,
                    "Only projects untouched for",
                    "Skip projects you worked on recently.",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut days)
                                .range(0..=3650)
                                .custom_formatter(|n, _| {
                                    if n == 0.0 {
                                        "any age".into()
                                    } else {
                                        format!("{n} days")
                                    }
                                })
                                .custom_parser(|s| {
                                    s.trim()
                                        .trim_end_matches("days")
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .or(Some(0.0))
                                }),
                        );
                    },
                );
                self.older_days = days;
                let mut mb = self.min_mb;
                setting_row(ui, p, "Hide items smaller than", "", |ui| {
                    ui.add(egui::DragValue::new(&mut mb).range(0..=100_000).suffix(" MB"));
                });
                self.min_mb = mb;
                let mut git = self.git;
                setting_row(
                    ui,
                    p,
                    "Check git status",
                    "Flag projects with uncommitted changes and merged worktrees.",
                    |ui| {
                        toggle(ui, &mut git, p);
                    },
                );
                self.git = git;
            });
            ui.add_space(12.0);

            // Cleaning
            card(ui, p, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("When cleaning").strong().color(p.text));
                ui.add_space(8.0);
                let mut trash = self.trash;
                setting_row(
                    ui,
                    p,
                    "Move to Trash instead of deleting",
                    "Undoable, but space is only freed after you empty the Trash.",
                    |ui| {
                        toggle(ui, &mut trash, p);
                    },
                );
                self.trash = trash;
                let mut smart = self.smart;
                setting_row(
                    ui,
                    p,
                    "Smart cache cleanup",
                    "Use pnpm store prune, brew cleanup and uv cache prune instead of wiping.",
                    |ui| {
                        toggle(ui, &mut smart, p);
                    },
                );
                self.smart = smart;
            });
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                if primary_button(ui, "  Save as defaults  ", p.accent, p.on_accent, true)
                    .clicked()
                {
                    let mut vals = vec![
                        (
                            "paths",
                            toml::Value::Array(
                                self.paths
                                    .iter()
                                    .map(|s| toml::Value::String(s.clone()))
                                    .collect(),
                            ),
                        ),
                        ("trash", toml::Value::Boolean(self.trash)),
                        ("smart_caches", toml::Value::Boolean(self.smart)),
                        ("min_size", toml::Value::String(format!("{}MB", self.min_mb))),
                    ];
                    if self.older_days > 0 {
                        vals.push((
                            "older_than",
                            toml::Value::String(format!("{}d", self.older_days)),
                        ));
                    }
                    self.settings_note = Some(match config::save_defaults(&home, &vals) {
                        Ok(path) => {
                            self.cfg = Config::load(&home).unwrap_or_default();
                            (format!("Saved to {}", pretty_path(&path, &home)), true)
                        }
                        Err(e) => (format!("Couldn't save: {e:#}"), false),
                    });
                }
                if ui
                    .add(egui::Button::new("Open config file").min_size(Vec2::new(0.0, 32.0)))
                    .clicked()
                {
                    let path = config::path(&home);
                    if !path.exists() {
                        let _ = Config::init(&home);
                    }
                    open_path(&path);
                }
                if let Some((msg, ok)) = &self.settings_note {
                    ui.label(
                        RichText::new(msg)
                            .size(12.0)
                            .color(if *ok { p.ok } else { p.warn }),
                    );
                }
            });
            ui.add_space(16.0);

            card(ui, p, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Background cleanup").strong().color(p.text));
                let installed = home
                    .join("Library/LaunchAgents/dev.diskdiet.agent.plist")
                    .exists()
                    || home.join(".config/systemd/user/diskdiet.timer").exists();
                ui.horizontal(|ui| {
                    let c = if installed { p.ok } else { p.faint };
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, c);
                    ui.label(
                        RichText::new(if installed {
                            "Installed"
                        } else {
                            "Not installed"
                        })
                        .size(12.0)
                        .color(p.dim),
                    );
                });
                ui.label(
                    RichText::new(
                        "Weekly cleanup of projects you haven't touched in 14 days, plus a notification when storage drops below 20 GB. Set it up from a terminal:",
                    )
                    .size(12.0)
                    .color(p.dim),
                );
                ui.add_space(6.0);
                let cmd = format!(
                    "diskdiet service install {} --alert-below 20GB",
                    self.paths.join(" ")
                );
                egui::Frame::new()
                    .fill(p.input)
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&cmd).monospace().color(p.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::icon_button(ui, Icon::Copy, p.faint, p.text, "Copy")
                                    .clicked()
                                {
                                    ui.ctx().copy_text(cmd.clone());
                                }
                            });
                        });
                    });
            });
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("diskdiet {VERSION} (beta)"))
                        .size(12.0)
                        .color(p.faint),
                );
                ui.hyperlink_to(
                    RichText::new("GitHub").size(12.0),
                    "https://github.com/anggerari/diskdiet",
                );
                ui.hyperlink_to(
                    RichText::new("Report an issue").size(12.0),
                    "https://github.com/anggerari/diskdiet/issues",
                );
            });
        });
    }
}

/// Minimal 24-bit BMP writer (no image crate needed).
fn write_bmp(path: &Path, img: &egui::ColorImage) -> std::io::Result<()> {
    let [w, h] = img.size;
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let c = img.pixels[y * w + x];
            b.extend_from_slice(&[c.b(), c.g(), c.r()]);
        }
        b.resize(b.len() + row - w * 3, 0);
    }
    std::fs::write(path, b)
}
