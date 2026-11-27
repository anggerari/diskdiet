//! Colors and widget styling. One accent (a calm green: "diet"), red only for
//! destructive actions, amber only for warnings. Everything else is neutral.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Theme, Vec2};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub input: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub ok: Color32,
    pub hover: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color32::from_rgb(0x16, 0x17, 0x1a),
            panel: Color32::from_rgb(0x1b, 0x1c, 0x20),
            card: Color32::from_rgb(0x22, 0x23, 0x28),
            input: Color32::from_rgb(0x12, 0x13, 0x16),
            border: Color32::from_rgb(0x2e, 0x30, 0x36),
            text: Color32::from_rgb(0xe6, 0xe7, 0xea),
            dim: Color32::from_rgb(0xa3, 0xa6, 0xad),
            faint: Color32::from_rgb(0x6e, 0x72, 0x7a),
            accent: Color32::from_rgb(0x59, 0xc2, 0x7f),
            on_accent: Color32::from_rgb(0x0c, 0x1f, 0x13),
            danger: Color32::from_rgb(0xe5, 0x53, 0x4b),
            warn: Color32::from_rgb(0xd9, 0xa2, 0x3a),
            ok: Color32::from_rgb(0x59, 0xc2, 0x7f),
            hover: Color32::from_rgb(0x25, 0x27, 0x2c),
        }
    } else {
        Palette {
            bg: Color32::from_rgb(0xff, 0xff, 0xff),
            panel: Color32::from_rgb(0xf6, 0xf7, 0xf8),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            input: Color32::from_rgb(0xf1, 0xf2, 0xf4),
            border: Color32::from_rgb(0xe3, 0xe5, 0xe8),
            text: Color32::from_rgb(0x1f, 0x23, 0x28),
            dim: Color32::from_rgb(0x59, 0x60, 0x6a),
            faint: Color32::from_rgb(0x8b, 0x92, 0x9c),
            accent: Color32::from_rgb(0x1f, 0x8a, 0x4c),
            on_accent: Color32::WHITE,
            danger: Color32::from_rgb(0xcf, 0x2f, 0x27),
            warn: Color32::from_rgb(0x9a, 0x67, 0x00),
            ok: Color32::from_rgb(0x1f, 0x8a, 0x4c),
            hover: Color32::from_rgb(0xf3, 0xf4, 0xf6),
        }
    }
}

pub fn current(ctx: &egui::Context) -> Palette {
    palette(ctx.global_style().visuals.dark_mode)
}

pub fn apply(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        let p = palette(theme == Theme::Dark);
        ctx.style_mut_of(theme, |s| {
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
            s.spacing.button_padding = Vec2::new(10.0, 5.0);
            s.spacing.interact_size.y = 26.0;
            s.text_styles
                .insert(TextStyle::Body, FontId::proportional(13.5));
            s.text_styles
                .insert(TextStyle::Button, FontId::proportional(13.0));
            s.text_styles
                .insert(TextStyle::Small, FontId::proportional(11.0));
            s.text_styles
                .insert(TextStyle::Monospace, FontId::monospace(12.5));
            s.text_styles
                .insert(TextStyle::Heading, FontId::proportional(19.0));

            let v = &mut s.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.card;
            v.extreme_bg_color = p.input;
            v.faint_bg_color = p.hover;
            v.window_stroke = Stroke::new(1.0, p.border);
            v.window_corner_radius = CornerRadius::same(10);
            v.menu_corner_radius = CornerRadius::same(8);
            v.selection.bg_fill = p.accent.gamma_multiply(0.07);
            v.selection.stroke = Stroke::new(1.0, p.accent);
            v.hyperlink_color = p.accent;
            v.override_text_color = None;

            let r = CornerRadius::same(6);
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = r;
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
            v.widgets.inactive.weak_bg_fill = p.card;
            v.widgets.inactive.bg_fill = p.card;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
            v.widgets.hovered.weak_bg_fill = p.hover;
            v.widgets.hovered.bg_fill = p.hover;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.faint);
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
            v.widgets.active.weak_bg_fill = p.border;
            v.widgets.active.bg_fill = p.border;
            v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
        });
    }
}

/// Muted brand-ish color per ecosystem, used only for the small badge.
pub fn kind_color(kind: &str) -> Color32 {
    let (r, g, b) = match kind {
        "Node" => (0x5f, 0xa0, 0x4e),
        "Next.js" | "Turborepo" => (0x9a, 0x9a, 0x9a),
        "Nuxt" => (0x00, 0xb3, 0x7a),
        "SvelteKit" => (0xe8, 0x5d, 0x2f),
        "Angular" => (0xd6, 0x3b, 0x4a),
        "Parcel" => (0xd9, 0x9a, 0x3a),
        "Rust" => (0xce, 0x7c, 0x4a),
        "Maven" | "Gradle" => (0x6d, 0x8f, 0xb3),
        "Flutter" => (0x47, 0xa8, 0xdb),
        "CocoaPods" => (0xd9, 0x4c, 0x3d),
        "SwiftPM" => (0xf0, 0x6a, 0x3a),
        "Python" | "Python venv" => (0x4b, 0x8b, 0xbe),
        "Zig" => (0xe0, 0xa0, 0x30),
        "Haskell" => (0x8f, 0x7f, 0xbf),
        "Elixir" => (0x8e, 0x6b, 0xb5),
        "Elm" => (0x60, 0xb5, 0xcc),
        "Terraform" => (0x7b, 0x6b, 0xd6),
        _ => (0x8a, 0x90, 0x99),
    };
    Color32::from_rgb(r, g, b)
}

pub fn monogram(kind: &str) -> String {
    match kind {
        "Node" => "JS",
        "Next.js" => "N",
        "Nuxt" => "Nu",
        "SvelteKit" => "Sv",
        "Turborepo" => "Tb",
        "Parcel" => "Pa",
        "Angular" => "Ng",
        "Rust" => "Rs",
        "Maven" => "Mv",
        "Gradle" => "Gr",
        "Flutter" => "Fl",
        "CocoaPods" => "Po",
        "SwiftPM" => "Sw",
        "Python" | "Python venv" => "Py",
        "Zig" => "Zg",
        "Haskell" => "Hs",
        "Elixir" => "Ex",
        "Elm" => "El",
        "Terraform" => "Tf",
        other => return other.chars().take(2).collect(),
    }
    .to_string()
}

pub const DOCKER_BLUE: Color32 = Color32::from_rgb(0x3a, 0x8f, 0xd9);
pub const CACHE_SLATE: Color32 = Color32::from_rgb(0x8a, 0x93, 0xa6);
pub const FILE_TAN: Color32 = Color32::from_rgb(0xb8, 0x93, 0x5a);
