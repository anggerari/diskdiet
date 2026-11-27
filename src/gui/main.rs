//! diskdiet desktop app (beta).
//!
//! A small native window on top of the same engine as the CLI. Design goal:
//! look like a system utility, not a landing page. Neutral colors that follow
//! the OS theme; color only where it carries meaning (destructive action,
//! warnings, the storage meter).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, RichText, Sense, Stroke, Ui, Vec2,
};
use egui_extras::{Column, TableBuilder};

use diskdiet::config::Config;
use diskdiet::engine::{self, Category, DeleteOptions, Item, Options, Outcome};
use diskdiet::util::{disk_space, expand_tilde, human_bytes};
use diskdiet::{stats, VERSION};

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
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([760.0, 460.0]),
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
                        .map(|a| diskdiet::util::pretty_path(std::path::Path::new(a), &app.home))
                        .collect::<Vec<_>>()
                        .join(", ");
                }
                app.start_scan(&cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Pre-rendered strings for one row so the table doesn't allocate per frame.
struct RowView {
    category: Category,
    kind: String,
    age: String,
    size: u64,
    size_label: String,
    label: String,
    label_lower: String,
    note: Option<String>,
    warn: bool,
    reveal: Option<PathBuf>,
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

struct App {
    home: PathBuf,
    cfg: Config,
    // Settings
    paths: String,
    older_days: u32,
    include: [bool; 4],
    trash: bool,
    smart: bool,
    // Results
    items: Arc<Vec<Item>>,
    rows: Vec<RowView>,
    selected: Vec<bool>,
    gone: Vec<bool>,
    max_size: u64,
    scanned: bool,
    warnings: Vec<String>,
    // View
    tab: Option<Category>,
    search: String,
    confirm: bool,
    phase: Phase,
    shared: Arc<Mutex<Shared>>,
    status: Option<(String, bool)>,
    disk: Option<(u64, u64)>,
    all_time: u64,
    /// Dev only: `DISKDIET_SNAPSHOT=out.bmp` saves a screenshot after the
    /// first scan and quits (used to make README images).
    snapshot: Option<(PathBuf, u32)>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        style(ctx);
        let home = dirs::home_dir().unwrap_or_default();
        let cfg = Config::load(&home).unwrap_or_default();
        let paths = if cfg.paths.is_empty() {
            "~".to_string()
        } else {
            cfg.paths.join(", ")
        };
        App {
            disk: disk_space(&home),
            all_time: stats::load().total_freed,
            trash: cfg.trash,
            smart: cfg.smart_caches.unwrap_or(true),
            home,
            cfg,
            paths,
            older_days: 0,
            include: [true, true, true, false],
            items: Arc::new(Vec::new()),
            rows: Vec::new(),
            selected: Vec::new(),
            gone: Vec::new(),
            max_size: 1,
            scanned: false,
            warnings: Vec::new(),
            tab: None,
            search: String::new(),
            confirm: false,
            phase: Phase::Idle,
            shared: Arc::default(),
            status: None,
            snapshot: std::env::var_os("DISKDIET_SNAPSHOT").map(|p| (PathBuf::from(p), 0)),
        }
    }

    fn options(&self) -> Options {
        let mut o = Options::from_config(&self.cfg, &self.home);
        o.paths = self
            .paths
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| expand_tilde(p, &self.home))
            .collect();
        o.older_than = (self.older_days > 0)
            .then(|| std::time::Duration::from_secs(self.older_days as u64 * 86_400));
        [o.projects, o.caches, o.docker, o.big] = self.include;
        o
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        self.phase = Phase::Scanning;
        self.status = None;
        self.confirm = false;
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

    fn start_delete(&mut self, ctx: &egui::Context) {
        let idx: Vec<usize> = self.visible_selected();
        if idx.is_empty() {
            return;
        }
        self.phase = Phase::Deleting;
        self.confirm = false;
        let items = self.items.clone();
        let home = self.home.clone();
        let opts = DeleteOptions {
            trash: self.trash,
            smart_caches: self.smart,
        };
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
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
                .map(|i| {
                    let label = i.label(&self.home);
                    let note = i.note(self.smart);
                    RowView {
                        category: i.category(),
                        kind: i.kind_label(),
                        age: i.age_label(),
                        size: i.size,
                        size_label: human_bytes(i.size),
                        label_lower: label.to_lowercase(),
                        label,
                        warn: note.as_deref().is_some_and(|n| n.contains('⚠'))
                            || matches!(&i.what, engine::What::Docker(o) if o.tier == diskdiet::docker::Tier::Volume),
                        note,
                        reveal: i.reveal_path().map(|p| p.to_path_buf()),
                    }
                })
                .collect();
            self.selected = scan.items.iter().map(|i| i.default_selected()).collect();
            self.gone = vec![false; scan.items.len()];
            self.max_size = scan.items.iter().map(|i| i.size).max().unwrap_or(1).max(1);
            self.items = Arc::new(scan.items);
            self.scanned = true;
            self.phase = Phase::Idle;
            self.disk = disk_space(&self.home);
        }
        if let Some(done) = s.done.take() {
            for &i in &done.removed {
                self.gone[i] = true;
                self.selected[i] = false;
            }
            self.disk = disk_space(&self.home);
            self.all_time = stats::load().total_freed;
            let o = &done.outcome;
            let mut msg = format!("Freed {}", human_bytes(o.freed));
            if o.trashed > 0 {
                msg += &format!(
                    ", moved {} to the Trash (empty it to get the space back)",
                    human_bytes(o.trashed)
                );
            }
            if let (Some(b), Some((_, after))) = (done.before, self.disk) {
                msg += &format!(". Free space {} → {}", human_bytes(b), human_bytes(after));
            }
            if !o.errors.is_empty() {
                msg += &format!(". {} item(s) failed: {}", o.errors.len(), o.errors[0]);
            }
            self.status = Some((msg, !o.errors.is_empty()));
            self.phase = Phase::Idle;
        }
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        (0..self.rows.len())
            .filter(|&i| !self.gone[i])
            .filter(|&i| self.tab.is_none_or(|t| self.rows[i].category == t))
            .filter(|&i| q.is_empty() || self.rows[i].label_lower.contains(&q))
            .collect()
    }

    fn visible_selected(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&i| !self.gone[i] && self.selected[i])
            .collect()
    }

    fn category_totals(&self, c: Option<Category>) -> (usize, u64) {
        (0..self.rows.len())
            .filter(|&i| !self.gone[i] && c.is_none_or(|c| self.rows[i].category == c))
            .fold((0, 0), |(n, b), i| (n + 1, b + self.rows[i].size))
    }
}

impl App {
    fn snapshot_tick(&mut self, ctx: &egui::Context) {
        let Some((path, frames)) = &mut self.snapshot else {
            return;
        };
        if !self.scanned || self.phase != Phase::Idle {
            return;
        }
        *frames += 1;
        ctx.request_repaint();
        if *frames == 5 {
            let path = path.clone();
            let c = ctx.clone();
            ctx.request_screenshot(move |img| {
                let _ = write_bmp(&path, &img);
                c.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }
}

/// Minimal 24-bit BMP writer (no image crate needed).
fn write_bmp(path: &std::path::Path, img: &egui::ColorImage) -> std::io::Result<()> {
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

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(13.5));
        s.text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.5));
        s.text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(11.5));
        s.text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(17.0));
        s.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(4);
        s.visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
        s.visuals.widgets.hovered.corner_radius = CornerRadius::same(4);
        s.visuals.widgets.active.corner_radius = CornerRadius::same(4);
    });
}

fn danger(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(220, 80, 70)
    } else {
        Color32::from_rgb(190, 40, 35)
    }
}

fn warn(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(230, 170, 60)
    } else {
        Color32::from_rgb(160, 100, 0)
    }
}

/// Small outlined "Beta" tag. Outline, not a filled pill, so it reads as a
/// status label and doesn't compete with the content.
fn beta_tag(ui: &mut Ui) {
    let text = "BETA";
    let font = FontId::proportional(10.0);
    let color = warn(ui);
    let galley = ui.painter().layout_no_wrap(text.into(), font, color);
    let size = galley.size() + Vec2::new(10.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_stroke(rect, 3.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// Thin storage meter: used part filled, free part empty.
fn storage_meter(ui: &mut Ui, total: u64, free: u64) {
    let used = total.saturating_sub(free) as f32 / total.max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(160.0, 6.0), Sense::hover());
    let p = ui.painter();
    let bg = ui.visuals().widgets.inactive.bg_fill;
    p.rect_filled(rect, 3.0, bg);
    let mut fill = rect;
    fill.set_width(rect.width() * used);
    let color = if used > 0.9 {
        danger(ui)
    } else {
        ui.visuals().text_color()
    };
    p.rect_filled(fill, 3.0, color.gamma_multiply(0.75));
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        self.snapshot_tick(&ctx);
        let busy = self.phase != Phase::Idle;

        // ── Header ──────────────────────────────────────────────────────
        egui::Panel::top("header")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("diskdiet").heading().strong());
                    beta_tag(ui);
                    ui.label(
                        RichText::new(VERSION)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    )
                    .on_hover_text("diskdiet is in beta. Review the list before deleting.");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some((total, free)) = self.disk {
                            ui.label(format!(
                                "{} free of {}",
                                human_bytes(free),
                                human_bytes(total)
                            ));
                            storage_meter(ui, total, free);
                        }
                        if self.all_time > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "{} freed so far",
                                    human_bytes(self.all_time)
                                ))
                                .color(ui.visuals().weak_text_color()),
                            );
                            ui.separator();
                        }
                    });
                });
            });

        // ── Footer: selection + action ─────────────────────────────────
        egui::Panel::bottom("footer")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let sel = self.visible_selected();
                    let bytes: u64 = sel.iter().map(|&i| self.rows[i].size).sum();
                    match self.phase {
                        Phase::Scanning => {
                            ui.spinner();
                            ui.label(self.shared.lock().unwrap().message.clone());
                        }
                        Phase::Deleting => {
                            let (d, t) = self.shared.lock().unwrap().progress;
                            ui.add(
                                egui::ProgressBar::new(d as f32 / t.max(1) as f32)
                                    .desired_width(220.0)
                                    .text(format!("{d} / {t}")),
                            );
                        }
                        Phase::Idle => {
                            if let Some((msg, failed)) = &self.status {
                                let c = if *failed {
                                    warn(ui)
                                } else {
                                    ui.visuals().text_color()
                                };
                                ui.label(RichText::new(msg).color(c));
                            } else if self.scanned {
                                ui.label(format!(
                                    "{} selected · {}",
                                    sel.len(),
                                    human_bytes(bytes)
                                ));
                            }
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let verb = if self.trash {
                            "Move to Trash"
                        } else {
                            "Delete"
                        };
                        let text = if sel.is_empty() {
                            verb.to_string()
                        } else {
                            format!("{verb} {} item(s)…", sel.len())
                        };
                        let button = egui::Button::new(RichText::new(text).color(Color32::WHITE))
                            .fill(danger(ui));
                        if ui.add_enabled(!busy && !sel.is_empty(), button).clicked() {
                            self.confirm = true;
                        }
                    });
                });
            });

        // ── Sidebar: categories + scan settings ────────────────────────
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(230.0)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::same(12)))
            .show(ui, |ui| {
              egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new("Show").small().color(ui.visuals().weak_text_color()));
                let mut tabs: Vec<(Option<Category>, &str)> = vec![(None, "Everything")];
                tabs.extend(CATEGORIES.iter().map(|c| (Some(*c), c.label())));
                for (cat, name) in tabs {
                    let (n, b) = self.category_totals(cat);
                    let active = self.tab == cat;
                    let resp = ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::selectable(active, "").frame_when_inactive(false),
                    );
                    // Name left, size right, inside the button rect.
                    let r = resp.rect.shrink2(Vec2::new(8.0, 0.0));
                    let color = ui.visuals().text_color();
                    let weak = ui.visuals().weak_text_color();
                    ui.painter().text(
                        r.left_center(),
                        egui::Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(13.5),
                        color,
                    );
                    if self.scanned {
                        ui.painter().text(
                            r.right_center(),
                            egui::Align2::RIGHT_CENTER,
                            format!("{} · {n}", human_bytes(b)),
                            FontId::proportional(11.5),
                            weak,
                        );
                    }
                    if resp.clicked() {
                        self.tab = cat;
                    }
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("Scan").small().color(ui.visuals().weak_text_color()));
                ui.add_enabled_ui(!busy, |ui| {
                    ui.label("Folders (comma separated)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.paths)
                            .desired_width(f32::INFINITY)
                            .hint_text("~/code"),
                    );
                    ui.add_space(4.0);
                    for (i, c) in CATEGORIES.iter().enumerate() {
                        ui.checkbox(&mut self.include[i], c.label());
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Untouched for");
                        ui.add(
                            egui::DragValue::new(&mut self.older_days)
                                .range(0..=3650)
                                .custom_formatter(|n, _| {
                                    if n == 0.0 { "any age".into() } else { format!("{n} days") }
                                })
                                .custom_parser(|s| {
                                    s.trim().trim_end_matches("days").trim().parse::<f64>().ok().or(Some(0.0))
                                }),
                        );
                    })
                    .response
                    .on_hover_text("Only list projects you haven't changed in this many days. 0 = any.");
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.trash, "Move to Trash instead of deleting")
                        .on_hover_text("Safer, but space comes back only after you empty the Trash.");
                    ui.checkbox(&mut self.smart, "Smart cache cleanup")
                        .on_hover_text("Use pnpm store prune, brew cleanup and uv cache prune instead of wiping those caches.");
                    ui.add_space(8.0);
                    let label = if self.scanned { "Scan again" } else { "Scan" };
                    if ui
                        .add_sized([ui.available_width(), 28.0], egui::Button::new(label))
                        .clicked()
                    {
                        self.start_scan(&ctx);
                    }
                });
              });
            });

        // ── Main: results table ────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(12)))
            .show(ui, |ui| {
                if !self.scanned {
                    ui.add_space(ui.available_height() * 0.35);
                    ui.vertical_centered(|ui| {
                        if self.phase == Phase::Scanning {
                            ui.spinner();
                            ui.label(self.shared.lock().unwrap().message.clone());
                        } else {
                            ui.label(RichText::new("See what's taking up your storage.").heading());
                            ui.label(
                                RichText::new("diskdiet lists node_modules, build folders, caches and Docker data. Nothing is deleted until you choose.")
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.add_space(10.0);
                            if ui.add(egui::Button::new("Scan").min_size(Vec2::new(120.0, 30.0))).clicked() {
                                self.start_scan(&ctx);
                            }
                        }
                    });
                    return;
                }

                let visible = self.visible();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Filter by path or name")
                            .desired_width(260.0),
                    );
                    ui.add_enabled_ui(!busy, |ui| {
                        if ui.button("Select all").clicked() {
                            visible.iter().for_each(|&i| self.selected[i] = true);
                        }
                        if ui.button("Select none").clicked() {
                            visible.iter().for_each(|&i| self.selected[i] = false);
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let total: u64 = visible.iter().map(|&i| self.rows[i].size).sum();
                        ui.label(
                            RichText::new(format!("{} items · {}", visible.len(), human_bytes(total)))
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                });
                for w in &self.warnings {
                    ui.label(RichText::new(w).small().color(warn(ui)));
                }
                ui.add_space(4.0);

                if visible.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Nothing here.").color(ui.visuals().weak_text_color()));
                    });
                    return;
                }

                let max = self.max_size as f32;
                let weak = ui.visuals().weak_text_color();
                let warn_c = warn(ui);
                let bar = ui.visuals().text_color().gamma_multiply(0.35);
                TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(26.0))
                    .column(Column::exact(96.0))
                    .column(Column::exact(104.0))
                    .column(Column::exact(48.0))
                    .column(Column::remainder().clip(true))
                    .column(Column::exact(52.0))
                    .header(22.0, |mut h| {
                        for t in ["", "Size", "Type", "Age", "Location", ""] {
                            h.col(|ui| {
                                ui.label(RichText::new(t).small().color(weak));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(30.0, visible.len(), |mut row| {
                            let i = visible[row.index()];
                            let r = &self.rows[i];
                            row.col(|ui| {
                                ui.add_enabled(!busy, egui::Checkbox::without_text(&mut self.selected[i]));
                            });
                            row.col(|ui| {
                                // Size with a proportional bar underneath.
                                let rect = ui.max_rect();
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(&r.size_label).monospace());
                                });
                                let w = (r.size as f32 / max).sqrt() * (rect.width() - 6.0);
                                let y = rect.bottom() - 4.0;
                                ui.painter().line_segment(
                                    [egui::pos2(rect.right() - w.max(2.0), y), egui::pos2(rect.right(), y)],
                                    Stroke::new(2.0, bar),
                                );
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(&r.kind).color(weak));
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(&r.age).color(weak));
                            });
                            row.col(|ui| {
                                ui.label(&r.label).on_hover_text(&r.label);
                                if let Some(n) = &r.note {
                                    let c = if r.warn { warn_c } else { weak };
                                    ui.label(RichText::new(n).small().color(c));
                                }
                            });
                            row.col(|ui| {
                                if let Some(p) = &r.reveal {
                                    if ui.small_button("Show").on_hover_text("Show in Finder").clicked() {
                                        engine::reveal(p);
                                    }
                                }
                            });
                        });
                    });
            });

        // ── Confirm dialog ─────────────────────────────────────────────
        if self.confirm {
            let sel = self.visible_selected();
            let bytes: u64 = sel.iter().map(|&i| self.rows[i].size).sum();
            let docker = sel
                .iter()
                .filter(|&&i| self.rows[i].category == Category::Docker)
                .count();
            let risky = sel.iter().filter(|&&i| self.rows[i].warn).count();
            let modal = egui::Modal::new(egui::Id::unique("confirm")).show(&ctx, |ui| {
                ui.set_width(380.0);
                let verb = if self.trash {
                    "Move to Trash"
                } else {
                    "Delete"
                };
                ui.label(
                    RichText::new(format!("{verb} {} item(s)?", sel.len()))
                        .heading()
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(format!("About {} will be removed.", human_bytes(bytes)));
                if self.trash {
                    ui.label(
                        RichText::new("Space is freed after you empty the Trash.")
                            .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    ui.label(
                        RichText::new(
                            "This can't be undone. Build folders and caches can be regenerated.",
                        )
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                if docker > 0 {
                    ui.label(
                        RichText::new(format!("{docker} Docker item(s) are removed permanently."))
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                if risky > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{risky} item(s) have a warning (uncommitted changes or volume data)."
                        ))
                        .color(warn(ui)),
                    );
                }
                ui.add_space(10.0);
                let mut go = false;
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let b = egui::Button::new(RichText::new(verb).color(Color32::WHITE))
                            .fill(danger(ui));
                        go = ui.add(b).clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
                (go, cancel)
            });
            let (go, cancel) = modal.inner;
            if go {
                self.start_delete(&ctx);
            } else if cancel || modal.should_close() {
                self.confirm = false;
            }
        }
    }
}
