//! A small set of line icons drawn with the painter (no icon font needed).
//! Coordinates are in a 0..1 box inside the icon rect.

use std::f32::consts::TAU;

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Grid,
    Folder,
    Layers,
    Cube,
    File,
    Chart,
    Gear,
    Search,
    Reveal,
    Trash,
    Check,
    Close,
    ChevronRight,
    ChevronDown,
    Warning,
    Copy,
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center() - Vec2::splat(s / 2.0);
    let p = |x: f32, y: f32| Pos2::new(o.x + x * s, o.y + y * s);
    let stroke = Stroke::new((s / 11.0).clamp(1.2, 2.0), color);
    let line = |pts: Vec<Pos2>| painter.add(Shape::line(pts, stroke));
    let closed = |pts: Vec<Pos2>| painter.add(Shape::closed_line(pts, stroke));

    match icon {
        Icon::Grid => {
            for (x, y) in [(0.15, 0.15), (0.55, 0.15), (0.15, 0.55), (0.55, 0.55)] {
                painter.rect_stroke(
                    Rect::from_min_max(p(x, y), p(x + 0.3, y + 0.3)),
                    2.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Folder => {
            closed(vec![
                p(0.1, 0.25),
                p(0.42, 0.25),
                p(0.5, 0.35),
                p(0.9, 0.35),
                p(0.9, 0.8),
                p(0.1, 0.8),
            ]);
        }
        Icon::Layers => {
            closed(vec![p(0.5, 0.12), p(0.9, 0.32), p(0.5, 0.52), p(0.1, 0.32)]);
            line(vec![p(0.1, 0.5), p(0.5, 0.7), p(0.9, 0.5)]);
            line(vec![p(0.1, 0.68), p(0.5, 0.88), p(0.9, 0.68)]);
        }
        Icon::Cube => {
            closed(vec![
                p(0.5, 0.1),
                p(0.87, 0.3),
                p(0.87, 0.7),
                p(0.5, 0.9),
                p(0.13, 0.7),
                p(0.13, 0.3),
            ]);
            line(vec![p(0.13, 0.3), p(0.5, 0.5), p(0.87, 0.3)]);
            line(vec![p(0.5, 0.5), p(0.5, 0.9)]);
        }
        Icon::File => {
            closed(vec![
                p(0.22, 0.1),
                p(0.6, 0.1),
                p(0.8, 0.3),
                p(0.8, 0.9),
                p(0.22, 0.9),
            ]);
            line(vec![p(0.6, 0.1), p(0.6, 0.3), p(0.8, 0.3)]);
        }
        Icon::Chart => {
            line(vec![p(0.12, 0.88), p(0.88, 0.88)]);
            for (x, h) in [(0.27, 0.35), (0.5, 0.65), (0.73, 0.5)] {
                painter.rect_filled(
                    Rect::from_min_max(p(x - 0.07, 0.82 - h), p(x + 0.07, 0.82)),
                    1.5,
                    color,
                );
            }
        }
        Icon::Gear => {
            let c = p(0.5, 0.5);
            painter.circle_stroke(c, s * 0.14, stroke);
            painter.circle_stroke(c, s * 0.3, stroke);
            for i in 0..8 {
                let a = i as f32 * TAU / 8.0;
                let (sn, cs) = a.sin_cos();
                painter.line_segment(
                    [
                        Pos2::new(c.x + cs * s * 0.3, c.y + sn * s * 0.3),
                        Pos2::new(c.x + cs * s * 0.42, c.y + sn * s * 0.42),
                    ],
                    stroke,
                );
            }
        }
        Icon::Search => {
            painter.circle_stroke(p(0.43, 0.43), s * 0.26, stroke);
            line(vec![p(0.62, 0.62), p(0.86, 0.86)]);
        }
        Icon::Reveal => {
            line(vec![
                p(0.45, 0.18),
                p(0.18, 0.18),
                p(0.18, 0.82),
                p(0.82, 0.82),
                p(0.82, 0.55),
            ]);
            line(vec![p(0.48, 0.52), p(0.84, 0.16)]);
            line(vec![p(0.58, 0.16), p(0.84, 0.16), p(0.84, 0.42)]);
        }
        Icon::Trash => {
            line(vec![p(0.15, 0.26), p(0.85, 0.26)]);
            line(vec![
                p(0.38, 0.26),
                p(0.38, 0.13),
                p(0.62, 0.13),
                p(0.62, 0.26),
            ]);
            line(vec![
                p(0.24, 0.26),
                p(0.3, 0.88),
                p(0.7, 0.88),
                p(0.76, 0.26),
            ]);
            line(vec![p(0.43, 0.42), p(0.44, 0.72)]);
            line(vec![p(0.57, 0.42), p(0.56, 0.72)]);
        }
        Icon::Check => {
            line(vec![p(0.2, 0.52), p(0.42, 0.74), p(0.82, 0.3)]);
        }
        Icon::Close => {
            line(vec![p(0.25, 0.25), p(0.75, 0.75)]);
            line(vec![p(0.75, 0.25), p(0.25, 0.75)]);
        }
        Icon::ChevronRight => {
            line(vec![p(0.4, 0.25), p(0.65, 0.5), p(0.4, 0.75)]);
        }
        Icon::ChevronDown => {
            line(vec![p(0.25, 0.4), p(0.5, 0.65), p(0.75, 0.4)]);
        }
        Icon::Warning => {
            closed(vec![p(0.5, 0.12), p(0.9, 0.84), p(0.1, 0.84)]);
            line(vec![p(0.5, 0.4), p(0.5, 0.6)]);
            painter.circle_filled(p(0.5, 0.71), s * 0.04, color);
        }
        Icon::Copy => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.32, 0.32), p(0.85, 0.85)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(vec![
                p(0.2, 0.68),
                p(0.15, 0.68),
                p(0.15, 0.15),
                p(0.68, 0.15),
                p(0.68, 0.2),
            ]);
        }
    }
}

/// Paint an icon in-line, taking `size` x `size` of layout space.
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui.painter(), rect, icon, color);
    resp
}

/// Small square icon button with a hover background.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    color: Color32,
    hover: Color32,
    tooltip: &str,
) -> Response {
    let size = 26.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp.on_hover_text(tooltip);
    let c = if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
        hover
    } else {
        color
    };
    paint(ui.painter(), rect.shrink(6.0), icon, c);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// The app mark: storage as a donut chart, with the slice diskdiet gives
/// back pulled out and faded. Geometry matches docs/logo-*.svg (104x104).
pub fn mark(painter: &Painter, rect: Rect, accent: Color32, _bg: Color32) {
    let k = rect.width() / 104.0;
    let arc = |cx: f32, cy: f32, from: f32, to: f32| -> Vec<Pos2> {
        (0..=48)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / 48.0).to_radians();
                Pos2::new(
                    rect.left() + (cx + 35.0 * a.cos()) * k,
                    rect.top() + (cy + 35.0 * a.sin()) * k,
                )
            })
            .collect()
    };
    let width = 18.0 * k;
    painter.add(Shape::line(
        arc(52.0, 52.0, -25.0, 245.0),
        Stroke::new(width, accent),
    ));
    let (ox, oy) = (
        52.0 + 7.0 * (-70f32).to_radians().cos(),
        52.0 + 7.0 * (-70f32).to_radians().sin(),
    );
    painter.add(Shape::line(
        arc(ox, oy, -105.0, -35.0),
        Stroke::new(width, accent.gamma_multiply(0.45)),
    ));
}
