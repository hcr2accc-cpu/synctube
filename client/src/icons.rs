//! Vector icons painted with egui shapes (the bundled fonts have no symbol glyphs).

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Response, Shape, Stroke};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Icon {
    Previous,
    Play,
    Pause,
    Next,
    Up,
    Down,
    Remove,
    Volume,
    Mute,
    Check,
    Plus,
    Note,
    Search,
    PlayNext,
    Gear,
    ExitFullscreen,
}

/// A framed button showing `icon`, `size` points square.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, enabled: bool) -> Response {
    let resp = ui.add_enabled(enabled, egui::Button::new("").min_size(vec2(size, size)));
    let color = if enabled {
        ui.style().interact(&resp).fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let inner = resp.rect.shrink(size * 0.28);
    paint(ui.painter(), icon, inner, color);
    resp
}

/// A borderless icon button that lights up on hover.
pub fn ghost(ui: &mut egui::Ui, icon: Icon, size: f32, enabled: bool) -> Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), sense);
    let color = if !enabled {
        theme::TEXT_FAINT.gamma_multiply(0.5)
    } else if resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme::CARD_HOVER);
    }
    paint(ui.painter(), icon, rect.shrink(size * 0.27), color);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// The big round accent button (play/pause).
pub fn round(ui: &mut egui::Ui, icon: Icon, size: f32, enabled: bool) -> Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), sense);
    let fill = if !enabled {
        theme::CARD_HOVER
    } else if resp.hovered() && !resp.is_pointer_button_down_on() {
        theme::accent_hover()
    } else {
        theme::accent()
    };
    ui.painter().circle_filled(rect.center(), size / 2.0, fill);
    let mut inner = rect.shrink(size * 0.32);
    if icon == Icon::Play {
        inner = inner.translate(vec2(size * 0.03, 0.0));
    }
    let fg = if enabled {
        theme::ON_ACCENT
    } else {
        theme::TEXT_FAINT
    };
    paint(ui.painter(), icon, inner, fg);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// An icon without a button.
pub fn label(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint(ui.painter(), icon, rect.shrink(size * 0.12), color);
    resp
}

/// A small filled status dot.
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// The SyncTube mark: two offset rounded squares ("in sync") and a play triangle.
pub fn logo(painter: &egui::Painter, r: Rect) {
    let s = r.width();
    let back = Rect::from_min_size(r.min + vec2(s * 0.22, 0.0), vec2(s * 0.78, s * 0.78));
    let front = Rect::from_min_size(r.min + vec2(0.0, s * 0.22), vec2(s * 0.78, s * 0.78));
    painter.rect_filled(back, s * 0.18, theme::ACCENT_2);
    painter.rect_filled(front, s * 0.18, theme::ACCENT);
    let c = front.center();
    let h = s * 0.2;
    tri(
        painter,
        [
            pos2(c.x - h * 0.75, c.y - h),
            pos2(c.x + h, c.y),
            pos2(c.x - h * 0.75, c.y + h),
        ],
        Color32::WHITE,
    );
}

/// Window icon (RGBA, `size`×`size`) matching [`logo`].
pub fn app_icon(size: u32) -> egui::IconData {
    let s = size as f32;
    let side = s * 0.78;
    let rad = s * 0.18;
    let (fx, fy) = (0.0, s * 0.22);
    let (bx, by) = (s * 0.22, 0.0);
    let (tcx, tcy) = (fx + side / 2.0, fy + side / 2.0);
    let h = s * 0.2;
    let tri_pts = [
        (tcx - h * 0.75, tcy - h),
        (tcx + h, tcy),
        (tcx - h * 0.75, tcy + h),
    ];
    let rounded = |x: f32, y: f32, x0: f32, y0: f32| -> bool {
        if x < x0 || y < y0 || x > x0 + side || y > y0 + side {
            return false;
        }
        let cx = x.clamp(x0 + rad, x0 + side - rad);
        let cy = y.clamp(y0 + rad, y0 + side - rad);
        (x - cx).powi(2) + (y - cy).powi(2) <= rad * rad
    };
    let in_tri = |x: f32, y: f32| -> bool {
        let edge = |(x1, y1): (f32, f32), (x2, y2): (f32, f32)| {
            (x - x2) * (y1 - y2) - (x1 - x2) * (y - y2)
        };
        let d = [
            edge(tri_pts[0], tri_pts[1]),
            edge(tri_pts[1], tri_pts[2]),
            edge(tri_pts[2], tri_pts[0]),
        ];
        !(d.iter().any(|v| *v < 0.0) && d.iter().any(|v| *v > 0.0))
    };
    const SS: u32 = 4;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 3];
            let mut covered = 0.0f32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let c = if rounded(x, y, fx, fy) {
                        Some(if in_tri(x, y) {
                            Color32::WHITE
                        } else {
                            theme::ACCENT
                        })
                    } else if rounded(x, y, bx, by) {
                        Some(theme::ACCENT_2)
                    } else {
                        None
                    };
                    if let Some(c) = c {
                        acc[0] += c.r() as f32;
                        acc[1] += c.g() as f32;
                        acc[2] += c.b() as f32;
                        covered += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            let k = if covered > 0.0 { 1.0 / covered } else { 0.0 };
            rgba.extend_from_slice(&[
                (acc[0] * k) as u8,
                (acc[1] * k) as u8,
                (acc[2] * k) as u8,
                (covered / n * 255.0) as u8,
            ]);
        }
    }
    egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}

fn tri(painter: &egui::Painter, pts: [Pos2; 3], color: Color32) {
    painter.add(Shape::convex_polygon(pts.to_vec(), color, Stroke::NONE));
}

pub fn paint(painter: &egui::Painter, icon: Icon, r: Rect, color: Color32) {
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let (cx, cy) = (r.center().x, r.center().y);
    let w = r.width();
    let stroke = Stroke::new((w * 0.14).max(1.5), color);
    match icon {
        Icon::Play => tri(
            painter,
            [pos2(l + w * 0.1, t), pos2(rt, cy), pos2(l + w * 0.1, b)],
            color,
        ),
        Icon::Pause => {
            let bw = w * 0.3;
            painter.rect_filled(
                Rect::from_min_max(pos2(l + w * 0.08, t), pos2(l + w * 0.08 + bw, b)),
                1.5,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(rt - w * 0.08 - bw, t), pos2(rt - w * 0.08, b)),
                1.5,
                color,
            );
        }
        Icon::Next => {
            tri(
                painter,
                [pos2(l, t), pos2(rt - w * 0.18, cy), pos2(l, b)],
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(rt - w * 0.18, t), pos2(rt, b)),
                1.0,
                color,
            );
        }
        Icon::Previous => {
            tri(
                painter,
                [pos2(rt, t), pos2(l + w * 0.18, cy), pos2(rt, b)],
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(l, t), pos2(l + w * 0.18, b)),
                1.0,
                color,
            );
        }
        Icon::Up => tri(
            painter,
            [
                pos2(cx, t + w * 0.15),
                pos2(rt, b - w * 0.2),
                pos2(l, b - w * 0.2),
            ],
            color,
        ),
        Icon::Down => tri(
            painter,
            [
                pos2(l, t + w * 0.2),
                pos2(rt, t + w * 0.2),
                pos2(cx, b - w * 0.15),
            ],
            color,
        ),
        Icon::Remove => {
            let thin = Stroke::new((w * 0.12).max(1.4), color);
            painter.line_segment([pos2(l, t), pos2(rt, b)], thin);
            painter.line_segment([pos2(rt, t), pos2(l, b)], thin);
        }
        Icon::Check => {
            painter.line_segment([pos2(l, cy), pos2(l + w * 0.38, b - w * 0.1)], stroke);
            painter.line_segment(
                [pos2(l + w * 0.38, b - w * 0.1), pos2(rt, t + w * 0.1)],
                stroke,
            );
        }
        Icon::Plus => {
            painter.line_segment([pos2(cx, t), pos2(cx, b)], stroke);
            painter.line_segment([pos2(l, cy), pos2(rt, cy)], stroke);
        }
        Icon::Note => {
            let head = w * 0.2;
            let stem_x = l + w * 0.48;
            let thin = Stroke::new((w * 0.1).max(1.4), color);
            painter.circle_filled(pos2(stem_x - head * 0.85, b - head), head, color);
            painter.line_segment([pos2(stem_x, b - head), pos2(stem_x, t)], thin);
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(stem_x, t),
                    pos2(rt, t + w * 0.2),
                    pos2(rt, t + w * 0.4),
                    pos2(stem_x, t + w * 0.2),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Search => {
            let thin = Stroke::new((w * 0.12).max(1.4), color);
            let rad = w * 0.32;
            let c = pos2(l + rad + w * 0.04, t + rad + w * 0.04);
            painter.circle_stroke(c, rad, thin);
            painter.line_segment(
                [pos2(c.x + rad * 0.72, c.y + rad * 0.72), pos2(rt, b)],
                thin,
            );
        }
        Icon::PlayNext => {
            let thin = Stroke::new((w * 0.11).max(1.4), color);
            painter.line_segment([pos2(l, t + w * 0.2), pos2(l + w * 0.5, t + w * 0.2)], thin);
            painter.line_segment([pos2(l, cy), pos2(rt, cy)], thin);
            painter.line_segment([pos2(l, b - w * 0.1), pos2(rt, b - w * 0.1)], thin);
            tri(
                painter,
                [
                    pos2(l + w * 0.65, t),
                    pos2(rt, t + w * 0.2),
                    pos2(l + w * 0.65, t + w * 0.4),
                ],
                color,
            );
        }
        Icon::Gear => {
            let thin = Stroke::new((w * 0.13).max(1.4), color);
            let c = r.center();
            let ro = w * 0.5;
            let ri = w * 0.34;
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([c + d * ri, c + d * ro], Stroke::new(w * 0.2, color));
            }
            painter.circle_stroke(c, ri, thin);
            painter.circle_stroke(c, w * 0.13, thin);
        }
        Icon::ExitFullscreen => {
            // Four corners pointing inward.
            let k = w * 0.36;
            for (cx0, cy0, sx, sy) in [
                (l + k, t + k, -1.0, -1.0),
                (rt - k, t + k, 1.0, -1.0),
                (l + k, b - k, -1.0, 1.0),
                (rt - k, b - k, 1.0, 1.0),
            ] {
                let c = pos2(cx0, cy0);
                painter.line_segment([c, pos2(cx0 + sx * k, cy0)], stroke);
                painter.line_segment([c, pos2(cx0, cy0 + sy * k)], stroke);
            }
        }
        Icon::Volume | Icon::Mute => {
            let box_r = l + w * 0.3;
            painter.rect_filled(
                Rect::from_min_max(pos2(l, cy - w * 0.15), pos2(box_r, cy + w * 0.15)),
                0.0,
                color,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(box_r - 1.0, cy - w * 0.15),
                    pos2(l + w * 0.6, t),
                    pos2(l + w * 0.6, b),
                    pos2(box_r - 1.0, cy + w * 0.15),
                ],
                color,
                Stroke::NONE,
            ));
            let thin = Stroke::new((w * 0.08).max(1.2), color);
            if icon == Icon::Mute {
                let x0 = l + w * 0.72;
                let s = w * 0.14;
                painter.line_segment([pos2(x0, cy - s), pos2(rt, cy + s)], thin);
                painter.line_segment([pos2(x0, cy + s), pos2(rt, cy - s)], thin);
            } else {
                painter.line_segment(
                    [
                        pos2(l + w * 0.75, cy - w * 0.2),
                        pos2(l + w * 0.75, cy + w * 0.2),
                    ],
                    thin,
                );
                painter.line_segment([pos2(rt, cy - w * 0.35), pos2(rt, cy + w * 0.35)], thin);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Regenerates `client/assets/synctube.ico` (the exe icon) from [`super::app_icon`]:
    /// `cargo test -p client --lib write_ico -- --ignored`
    #[test]
    #[ignore]
    fn write_ico() {
        let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
        let images: Vec<Vec<u8>> = sizes
            .iter()
            .map(|&s| {
                let icon = super::app_icon(s);
                // BITMAPINFOHEADER, height doubled for the (empty) AND mask.
                let mut b = Vec::new();
                for v in [40u32, s, s * 2] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                b.extend_from_slice(&1u16.to_le_bytes());
                b.extend_from_slice(&32u16.to_le_bytes());
                b.extend_from_slice(&[0u8; 24]);
                // Bottom-up BGRA rows.
                for y in (0..s).rev() {
                    for x in 0..s {
                        let i = ((y * s + x) * 4) as usize;
                        let p = &icon.rgba[i..i + 4];
                        b.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                    }
                }
                let mask_row = s.div_ceil(32) * 4;
                b.resize(b.len() + (mask_row * s) as usize, 0);
                b
            })
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 0, 1, 0]);
        out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * sizes.len() as u32;
        for (&s, img) in sizes.iter().zip(&images) {
            let d = if s >= 256 { 0 } else { s as u8 };
            out.extend_from_slice(&[d, d, 0, 0]);
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&(img.len() as u32).to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            offset += img.len() as u32;
        }
        for img in &images {
            out.extend_from_slice(img);
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/synctube.ico");
        std::fs::write(path, out).unwrap();
    }
}
