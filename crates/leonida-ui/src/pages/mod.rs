mod assets;
mod home;
mod logs;
mod mods;
mod run;
mod settings;
pub mod shaders;
mod systems;
mod viewer;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

use crate::theme;

pub fn header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(4.0);
    ui.heading(RichText::new(title).strong());
    ui.weak(subtitle);
    ui.add_space(10.0);
}

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::card_fill(ui))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn card_title(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(16.0).strong());
    ui.add_space(4.0);
}

pub fn planned(ui: &mut egui::Ui, milestone: &str, what: &str) {
    egui::Frame::new()
        .fill(theme::PENDING.linear_multiply(0.08))
        .stroke(Stroke::new(1.0, theme::PENDING.linear_multiply(0.5)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("🚧 Not built yet")
                        .strong()
                        .color(theme::PENDING),
                );
                ui.label(format!(
                    "{what}. Arrives in {milestone} (docs/plans/2026-11-26-roadmap.md)."
                ));
            });
        });
}

pub fn later_button(ui: &mut egui::Ui, label: &str, milestone: &str) {
    ui.add_enabled(false, egui::Button::new(label))
        .on_disabled_hover_text(format!("Arrives in {milestone}"));
}

pub fn stat(ui: &mut egui::Ui, label: &str, value: impl ToString, color: Option<Color32>) {
    card(ui, |ui| {
        let mut text = RichText::new(value.to_string()).size(26.0).strong();
        if let Some(color) = color {
            text = text.color(color);
        }
        ui.label(text);
        ui.weak(label);
    });
}

pub fn check_line(ui: &mut egui::Ui, ok: bool, label: &str, detail: &str) {
    ui.horizontal(|ui| {
        if ok {
            ui.label(RichText::new("✔").color(theme::GOOD));
        } else {
            ui.label(RichText::new("✖").color(theme::BAD));
        }
        ui.label(label);
        if !detail.is_empty() {
            ui.weak(detail);
        }
    });
}
