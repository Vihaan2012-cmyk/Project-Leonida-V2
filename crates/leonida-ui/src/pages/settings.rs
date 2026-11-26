use eframe::egui;

use super::{card, card_title, header, later_button};
use crate::app::LeonidaApp;
use crate::theme;

impl LeonidaApp {
    pub(crate) fn settings_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Settings",
            "How Leonida looks and where it finds things.",
        );

        card(ui, |ui| {
            card_title(ui, "Appearance");
            ui.horizontal(|ui| {
                let before = self.dark;
                ui.radio_value(&mut self.dark, true, "Dark");
                ui.radio_value(&mut self.dark, false, "Light");
                if self.dark != before {
                    theme::set_dark(ui.ctx(), self.dark);
                }
            });
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            card_title(ui, "Game folder");
            match &self.game.path {
                Some(path) => {
                    ui.monospace(path.display().to_string());
                }
                None => {
                    ui.weak("None chosen.");
                }
            }
            ui.horizontal(|ui| {
                if ui.button("📂 Change…").clicked() {
                    self.pick_game_folder();
                }
                if ui
                    .add_enabled(
                        !self.jobs.is_busy(),
                        egui::Button::new("🔍 Find automatically"),
                    )
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.find_game(&ctx);
                }
            });
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            card_title(ui, "Build");
            let mut jobs = 8.0;
            ui.add_enabled(
                false,
                egui::Slider::new(&mut jobs, 1.0..=32.0).text("Parallel jobs"),
            )
            .on_disabled_hover_text("Arrives with shader translation (M1.8)");
            ui.horizontal(|ui| {
                ui.weak("Work folder: Output/");
                later_button(ui, "Change…", "M2.6");
            });
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            card_title(ui, "Input");
            ui.weak("Controller and keyboard bindings, saved to Config/input.toml.");
            later_button(ui, "Edit bindings…", "M4.4");
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            card_title(ui, "About");
            ui.label(format!("Leonida {}", env!("CARGO_PKG_VERSION")));
            ui.weak("GPL-3.0. Contains no game files: everything is built on your PC from your own copy.");
        });
    }
}
