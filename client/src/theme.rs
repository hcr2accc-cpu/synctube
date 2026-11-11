//! Colors, fonts and widget styling for the whole app.

use std::sync::Arc;

use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Margin, Stroke, TextStyle,
};

pub const BG: Color32 = Color32::from_rgb(0x10, 0x11, 0x16);
pub const PANEL: Color32 = Color32::from_rgb(0x15, 0x17, 0x1e);
pub const CARD: Color32 = Color32::from_rgb(0x1c, 0x1f, 0x28);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x24, 0x28, 0x33);
pub const FIELD: Color32 = Color32::from_rgb(0x0c, 0x0d, 0x12);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x3a);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xf0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xb0);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6a, 0x70, 0x80);
pub const ACCENT: Color32 = Color32::from_rgb(0x8b, 0x7b, 0xff);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xa0, 0x93, 0xff);
pub const ACCENT_2: Color32 = Color32::from_rgb(0xec, 0x5c, 0xa8);
pub const GOOD: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
pub const WARN: Color32 = Color32::from_rgb(0xfb, 0xbf, 0x24);
pub const BAD: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);

pub const RADIUS: u8 = 10;

/// Semi-bold font family (for titles and emphasis).
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, bold())
}

/// Accent color with the given alpha (0–255).
pub fn accent_alpha(a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), a)
}

/// A stable, pleasant color for a name (avatars, "added by").
pub fn name_color(name: &str) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in name.to_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.5, 0.92, 1.0).into()
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, style);
}

/// System fonts used as glyph fallbacks, in order: (name, candidate paths).
/// The first existing path of each entry is used.
#[cfg(windows)]
fn system_fallbacks() -> Vec<(&'static str, Vec<std::path::PathBuf>)> {
    let root = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let dir = std::path::Path::new(&root).join("Fonts");
    [
        ("sys-segoe", "segoeui.ttf"),
        ("sys-korean", "malgun.ttf"),
        ("sys-cjk", "msyh.ttc"),
        ("sys-symbols", "seguisym.ttf"),
    ]
    .into_iter()
    .map(|(name, file)| (name, vec![dir.join(file)]))
    .collect()
}

/// Common Linux locations (Debian/Ubuntu, Fedora, Arch).
#[cfg(not(windows))]
fn system_fallbacks() -> Vec<(&'static str, Vec<std::path::PathBuf>)> {
    let paths = |list: &[&str]| list.iter().map(std::path::PathBuf::from).collect();
    vec![
        (
            "sys-dejavu",
            paths(&[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
            ]),
        ),
        (
            "sys-cjk",
            paths(&[
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            ]),
        ),
    ]
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        &mut fonts,
        "inter",
        include_bytes!("../assets/fonts/inter-latin-400.ttf"),
    );
    add(
        &mut fonts,
        "inter-ext",
        include_bytes!("../assets/fonts/inter-latin-ext-400.ttf"),
    );
    add(
        &mut fonts,
        "inter-bold",
        include_bytes!("../assets/fonts/inter-latin-600.ttf"),
    );
    add(
        &mut fonts,
        "inter-bold-ext",
        include_bytes!("../assets/fonts/inter-latin-ext-600.ttf"),
    );
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    // Video titles can be in any script: fall back to the system's fonts
    // (Cyrillic/Greek, Korean, Chinese/Japanese) when they are installed.
    let mut system = Vec::new();
    for (name, candidates) in system_fallbacks() {
        if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
            fonts
                .font_data
                .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            system.push(name.to_string());
        }
    }
    let mut regular = vec!["inter".to_string(), "inter-ext".to_string()];
    regular.extend(system.iter().cloned());
    regular.extend(fallback.iter().cloned());
    let mut semibold = vec!["inter-bold".to_string(), "inter-bold-ext".to_string()];
    semibold.extend(system);
    semibold.extend(fallback);
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts.families.insert(bold(), semibold);
    ctx.set_fonts(fonts);
}

fn style(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Small, font(11.5)),
        (TextStyle::Body, font(14.0)),
        (TextStyle::Button, font(14.0)),
        (TextStyle::Heading, bold_font(22.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 8.0);
    s.button_padding = egui::vec2(12.0, 6.0);
    s.interact_size.y = 30.0;
    s.slider_rail_height = 5.0;
    s.window_margin = Margin::same(12);

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = None;
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = FIELD;
    v.text_edit_bg_color = Some(FIELD);
    v.faint_bg_color = CARD;
    v.hyperlink_color = ACCENT;
    v.error_fg_color = BAD;
    v.warn_fg_color = WARN;
    v.selection.bg_fill = accent_alpha(110);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.slider_trailing_fill = true;
    v.weak_text_color = Some(TEXT_DIM);

    let r = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = CARD_HOVER;
    w.inactive.weak_bg_fill = CARD;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = Color32::from_rgb(0x30, 0x34, 0x42);
    w.hovered.weak_bg_fill = CARD_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3d, 0x42, 0x52));
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = Color32::from_rgb(0x2c, 0x30, 0x3c);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open = w.active;
}
