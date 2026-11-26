use eframe::egui;

use super::{card, card_title, header, later_button, planned};
use crate::app::LeonidaApp;

impl LeonidaApp {
    pub(crate) fn mods_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Mods",
            "Packages layered over the game, like MSFS's Community folder. Later packages win.",
        );
        ui.horizontal(|ui| {
            later_button(ui, "➕ Add package…", "M2.6");
            later_button(ui, "📁 Open Mods folder", "M2.6");
            later_button(ui, "⚠ Check conflicts", "M2.6");
        });
        ui.add_space(8.0);
        planned(
            ui,
            "M2.6",
            "Package manifests, load order and conflict reports",
        );
        ui.add_space(8.0);
        card(ui, |ui| {
            card_title(ui, "Installed packages");
            egui::Grid::new("mods-header")
                .num_columns(5)
                .spacing([28.0, 6.0])
                .show(ui, |ui| {
                    for title in ["On", "Order", "Name", "Version", "Type"] {
                        ui.strong(title);
                    }
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.weak("No packages yet.");
        });
    }
}
