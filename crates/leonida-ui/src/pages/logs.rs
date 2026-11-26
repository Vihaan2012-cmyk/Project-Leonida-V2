use eframe::egui;

use super::header;
use crate::app::LeonidaApp;

impl LeonidaApp {
    pub(crate) fn logs_page(&mut self, ui: &mut egui::Ui) {
        header(ui, "Logs", "What Leonida has done this session.");
        ui.horizontal(|ui| {
            if ui.button("📋 Copy all").clicked() {
                ui.ctx().copy_text(self.logs.join("\n"));
            }
            if ui.button("🗑 Clear").clicked() {
                self.logs.clear();
            }
        });
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.logs.is_empty() {
                    ui.weak("Nothing yet.");
                }
                for line in &self.logs {
                    ui.monospace(line);
                }
            });
    }
}
