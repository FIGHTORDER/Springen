//! The Springen visual language, transcribed from `design/tokens/`.
//!
//! Chrome is a cool graphite ramp so the terrain is the only saturated thing on
//! screen. There is exactly **one** accent — contour orange — reserved for the
//! active or selected thing, the single primary action, and hero values in the
//! manifest. Shoal cyan is the secondary hue for engine-truth data. Saturated
//! colour otherwise belongs to the terrain, never to the interface.

// The full token set is transcribed, not only the tokens used so far: a
// partial palette is how design systems drift.
#![allow(dead_code)]

use eframe::egui::{self, Color32, CornerRadius, Stroke};

/* ---- neutral chrome (cool graphite) ---- */
pub const GRAY_1000: Color32 = Color32::from_rgb(0x07, 0x0A, 0x0C);
pub const GRAY_950: Color32 = Color32::from_rgb(0x0B, 0x0E, 0x11);
pub const GRAY_900: Color32 = Color32::from_rgb(0x12, 0x16, 0x1A);
pub const GRAY_850: Color32 = Color32::from_rgb(0x17, 0x1C, 0x21);
pub const GRAY_800: Color32 = Color32::from_rgb(0x1D, 0x23, 0x29);
pub const GRAY_750: Color32 = Color32::from_rgb(0x25, 0x2C, 0x33);
pub const GRAY_700: Color32 = Color32::from_rgb(0x30, 0x38, 0x40);
pub const GRAY_650: Color32 = Color32::from_rgb(0x3B, 0x44, 0x4D);
pub const RULE_1: Color32 = Color32::from_rgb(0x20, 0x27, 0x2D);
pub const RULE_2: Color32 = Color32::from_rgb(0x2B, 0x33, 0x3A);

/* ---- ink ---- */
pub const INK_100: Color32 = Color32::from_rgb(0xE4, 0xE9, 0xEE);
pub const INK_80: Color32 = Color32::from_rgb(0xC3, 0xCC, 0xD4);
pub const INK_60: Color32 = Color32::from_rgb(0x98, 0xA3, 0xAD);
pub const INK_45: Color32 = Color32::from_rgb(0x73, 0x7F, 0x8A);
pub const INK_30: Color32 = Color32::from_rgb(0x52, 0x5C, 0x65);

/* ---- the single accent ---- */
pub const CONTOUR_300: Color32 = Color32::from_rgb(0xF5, 0xB2, 0x7C);
pub const CONTOUR_400: Color32 = Color32::from_rgb(0xF0, 0xA0, 0x5A);
pub const CONTOUR_500: Color32 = Color32::from_rgb(0xE0, 0x8A, 0x3C);
pub const CONTOUR_600: Color32 = Color32::from_rgb(0xC4, 0x71, 0x2A);
pub const ON_CONTOUR: Color32 = Color32::from_rgb(0x17, 0x0E, 0x05);
pub const CONTOUR_TINT: Color32 = Color32::from_rgba_premultiplied(0x1E, 0x14, 0x0A, 0x1F);

/* ---- data / secondary ---- */
pub const SHOAL_400: Color32 = Color32::from_rgb(0x6F, 0xD3, 0xD8);
pub const SHOAL_500: Color32 = Color32::from_rgb(0x46, 0xB9, 0xC4);

/* ---- status, used only on validation ---- */
pub const ALERT_500: Color32 = Color32::from_rgb(0xDA, 0x5A, 0x4E);
pub const ALERT_300: Color32 = Color32::from_rgb(0xF0, 0xA7, 0x9F);
pub const GOOD_300: Color32 = Color32::from_rgb(0xA8, 0xD8, 0xB4);
pub const GOOD_500: Color32 = Color32::from_rgb(0x6F, 0xBF, 0x8B);
pub const WARN_300: Color32 = Color32::from_rgb(0xEB, 0xC7, 0x82);
pub const WARN_500: Color32 = Color32::from_rgb(0xD9, 0xA4, 0x41);

/* ---- semantic aliases ---- */
pub const SURFACE_APP: Color32 = GRAY_1000;
pub const SURFACE_CANVAS: Color32 = GRAY_950;
pub const SURFACE_CHROME: Color32 = GRAY_900;
pub const SURFACE_PANEL: Color32 = GRAY_850;
pub const SURFACE_RAISED: Color32 = GRAY_800;
pub const SURFACE_CONTROL: Color32 = GRAY_950;
pub const SURFACE_HOVER: Color32 = GRAY_800;
pub const BORDER_HAIRLINE: Color32 = RULE_1;
pub const BORDER_PANEL: Color32 = RULE_2;
pub const BORDER_CONTROL: Color32 = GRAY_700;
pub const BORDER_STRONG: Color32 = GRAY_650;
pub const TEXT_PRIMARY: Color32 = INK_100;
pub const TEXT_SECONDARY: Color32 = INK_60;
pub const TEXT_TERTIARY: Color32 = INK_45;
pub const TEXT_DISABLED: Color32 = INK_30;
pub const TEXT_DATA: Color32 = SHOAL_400;
pub const ACCENT: Color32 = CONTOUR_500;
pub const SCRIM: Color32 = Color32::from_rgba_premultiplied(5, 8, 9, 199);

/* ---- fixed chrome dimensions of the workstation shell ---- */
pub const TOOLBAR_H: f32 = 44.0;
pub const STATUSBAR_H: f32 = 24.0;
pub const PALETTE_W: f32 = 212.0;
pub const INSPECTOR_W: f32 = 344.0;
/// The live map beside the graph. Wide enough that a 12x12 map's shape reads
/// at a glance, narrow enough that it does not crowd the wiring out.
pub const GRAPH_MAP_W: f32 = 300.0;
pub const PANEL_PAD: f32 = 12.0;
pub const ROW_H: f32 = 24.0;
pub const CTL_H_SM: f32 = 22.0;
pub const CTL_H: f32 = 26.0;
pub const CTL_H_LG: f32 = 32.0;
pub const PANEL_HEADER_H: f32 = 32.0;

/// 0 for panels, rows and rails; 2px for controls and nodes; 3px for popovers.
pub const R_CONTROL: CornerRadius = CornerRadius::same(2);
pub const R_POPOVER: CornerRadius = CornerRadius::same(3);

/// The graph node is the one card-like object: 184px wide, with a 2px coloured
/// top edge encoding its class and a 22px drag header.
pub const NODE_W: f32 = 184.0;
pub const NODE_HEADER_H: f32 = 22.0;
pub const NODE_THUMB: f32 = 48.0;
pub const NODE_CLASS_EDGE: f32 = 2.0;

/// Canvas grid: 24px minor, 120px major hairlines.
pub const GRID_MINOR: f32 = 24.0;
pub const GRID_MAJOR: f32 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    /// Archivo 700 — wordmark and dialog titles only.
    Display,
    Ui,
    UiStrong,
    /// IBM Plex Mono, for every number, id, path and derived value.
    Mono,
}

pub fn font(role: FontRole, size: f32) -> egui::FontId {
    let family = match role {
        FontRole::Display => egui::FontFamily::Name("display".into()),
        FontRole::Ui => egui::FontFamily::Proportional,
        FontRole::UiStrong => egui::FontFamily::Name("strong".into()),
        FontRole::Mono => egui::FontFamily::Monospace,
    };
    egui::FontId::new(size, family)
}

/// Node class colours: grey operator, cyan terminal, olive texture.
pub fn class_colour(cat: &str) -> Color32 {
    match cat {
        "Output" => SHOAL_500,
        "Texture" => Color32::from_rgb(0x8A, 0x92, 0x54),
        "Erode" => Color32::from_rgb(0x9A, 0x7B, 0x5A),
        "Generate" => GRAY_650,
        _ => GRAY_700,
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
    };
    add(
        "plex-sans",
        include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    );
    add(
        "plex-sans-semibold",
        include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    );
    add(
        "plex-mono",
        include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    );
    add(
        "archivo",
        include_bytes!("../assets/fonts/Archivo-Bold.ttf"),
    );

    fonts
        .families
        .insert(egui::FontFamily::Proportional, vec!["plex-sans".into()]);
    fonts
        .families
        .insert(egui::FontFamily::Monospace, vec!["plex-mono".into()]);
    fonts.families.insert(
        egui::FontFamily::Name("strong".into()),
        vec!["plex-sans-semibold".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("display".into()),
        vec!["archivo".into()],
    );
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT_PRIMARY);
    v.panel_fill = SURFACE_PANEL;
    v.window_fill = SURFACE_PANEL;
    v.extreme_bg_color = SURFACE_CONTROL;
    v.faint_bg_color = SURFACE_RAISED;
    v.window_stroke = Stroke::new(1.0, BORDER_PANEL);
    v.window_corner_radius = R_POPOVER;
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(115),
    };
    v.window_shadow = v.popup_shadow;

    // Hover lightens one step; press darkens and nudges. No scale, no bounce.
    v.widgets.noninteractive.bg_fill = SURFACE_PANEL;
    v.widgets.noninteractive.weak_bg_fill = SURFACE_PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_HAIRLINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
    v.widgets.noninteractive.corner_radius = CornerRadius::ZERO;

    v.widgets.inactive.bg_fill = SURFACE_RAISED;
    v.widgets.inactive.weak_bg_fill = SURFACE_RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_CONTROL);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    v.widgets.inactive.corner_radius = R_CONTROL;

    v.widgets.hovered.bg_fill = SURFACE_HOVER;
    v.widgets.hovered.weak_bg_fill = GRAY_750;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    v.widgets.hovered.corner_radius = R_CONTROL;

    v.widgets.active.bg_fill = GRAY_900;
    v.widgets.active.weak_bg_fill = GRAY_900;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    v.widgets.active.corner_radius = R_CONTROL;

    v.widgets.open.bg_fill = SURFACE_RAISED;
    v.widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.open.corner_radius = R_CONTROL;

    v.selection.bg_fill = CONTOUR_TINT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    // Focus is a 1px accent ring at 1px offset.
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.expansion = 0.0;

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size = egui::vec2(0.0, CTL_H);
    style.spacing.window_margin = egui::Margin::same(PANEL_PAD as i8);
    style.spacing.slider_width = 150.0;
    style.spacing.scroll = egui::style::ScrollStyle::solid();

    style.text_styles = [
        (egui::TextStyle::Body, font(FontRole::Ui, 13.0)),
        (egui::TextStyle::Button, font(FontRole::Ui, 13.0)),
        (egui::TextStyle::Small, font(FontRole::Ui, 11.0)),
        (egui::TextStyle::Monospace, font(FontRole::Mono, 12.0)),
        (egui::TextStyle::Heading, font(FontRole::Display, 18.0)),
    ]
    .into();

    // 80ms hover, 120ms control state. Ease-out only, no springs.
    style.animation_time = 0.08;
    let style = std::sync::Arc::new(style);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

/// An 11px semibold uppercase micro-label at 0.09em tracking. Section titles
/// are the only place uppercase is used — never on a button.
pub fn micro_label(ui: &mut egui::Ui, text: &str) {
    let spaced: String = text
        .to_uppercase()
        .chars()
        .flat_map(|c| [c, '\u{2009}'])
        .collect();
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(spaced)
            .font(font(FontRole::UiStrong, 11.0))
            .color(TEXT_TERTIARY),
    );
    ui.add_space(2.0);
}

/// Mono values are always right-aligned.
pub fn stat_row(ui: &mut egui::Ui, label: &str, value: &str, hero: bool) {
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_H);
        ui.label(
            egui::RichText::new(label)
                .font(font(FontRole::Ui, 12.0))
                .color(TEXT_SECONDARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .font(font(FontRole::Mono, 12.0))
                    .color(if hero { ACCENT } else { TEXT_DATA }),
            );
        });
    });
}

pub fn hairline(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let y = ui.cursor().top();
    ui.painter()
        .hline(rect.x_range(), y, Stroke::new(1.0, BORDER_HAIRLINE));
    ui.add_space(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_dimensions_match_the_design_system() {
        // The window is a fixed frame; these are not negotiable per-screen.
        assert_eq!(TOOLBAR_H, 44.0);
        assert_eq!(PALETTE_W, 212.0);
        assert_eq!(INSPECTOR_W, 344.0);
        assert_eq!(STATUSBAR_H, 24.0);
        assert_eq!(NODE_W, 184.0);
    }

    #[test]
    fn nothing_is_rounder_than_five_pixels() {
        for r in [R_CONTROL, R_POPOVER] {
            assert!(r.nw <= 5 && r.ne <= 5 && r.sw <= 5 && r.se <= 5);
        }
    }

    #[test]
    fn there_is_exactly_one_accent() {
        // Terminal nodes use the secondary data hue, not a second accent.
        assert_eq!(class_colour("Output"), SHOAL_500);
        assert_ne!(class_colour("Generate"), ACCENT);
        assert_ne!(class_colour("Texture"), ACCENT);
    }
}
