use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Theme, ThemePreference, Vec2,
};

pub const ACCENT: Color32 = Color32::from_rgb(20, 184, 166);
pub const GOOD: Color32 = Color32::from_rgb(34, 197, 94);
pub const BAD: Color32 = Color32::from_rgb(239, 68, 68);
pub const PENDING: Color32 = Color32::from_rgb(234, 179, 8);

const UI_FONTS: &[&str] = &[
    "C:/Windows/Fonts/SegUIVar.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
];
const MONO_FONTS: &[&str] = &[
    "C:/Windows/Fonts/CascadiaMono.ttf",
    "C:/Windows/Fonts/consola.ttf",
];

pub fn install(ctx: &egui::Context, dark: bool) {
    ctx.set_fonts(fonts());
    ctx.style_mut_of(Theme::Dark, |style| shape(style, true));
    ctx.style_mut_of(Theme::Light, |style| shape(style, false));
    set_dark(ctx, dark);
}

pub fn set_dark(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (family, candidates, name) in [
        (FontFamily::Proportional, UI_FONTS, "system-ui"),
        (FontFamily::Monospace, MONO_FONTS, "system-mono"),
    ] {
        if let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) {
            fonts
                .font_data
                .insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, name.to_string());
        }
    }
    fonts
}

fn shape(style: &mut egui::Style, dark: bool) {
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(24.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 6.0);
    style.spacing.interact_size.y = 30.0;

    let visuals = &mut style.visuals;
    let radius = CornerRadius::same(8);
    visuals.window_corner_radius = CornerRadius::same(12);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
    }
    visuals.selection.bg_fill = ACCENT.linear_multiply(if dark { 0.55 } else { 0.35 });
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;
    if dark {
        visuals.panel_fill = Color32::from_rgb(17, 19, 23);
        visuals.window_fill = Color32::from_rgb(22, 24, 29);
        visuals.extreme_bg_color = Color32::from_rgb(11, 12, 15);
        visuals.faint_bg_color = Color32::from_rgb(26, 29, 35);
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(32, 35, 42);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(42, 46, 55);
    }
}

pub fn card_fill(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(24, 27, 33)
    } else {
        Color32::from_rgb(246, 247, 249)
    }
}
