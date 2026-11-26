use eframe::egui::{self, RichText};

use super::{card, card_title, check_line, header, later_button, planned};
use crate::app::LeonidaApp;

fn requirements(app: &LeonidaApp) -> Vec<(bool, String, String)> {
    let exe = app
        .game
        .exe_name
        .clone()
        .unwrap_or_else(|| "the game executable".to_string());
    vec![
        (
            app.game.is_supported(),
            "A supported copy of the game".to_string(),
            if app.game.is_supported() {
                String::new()
            } else {
                "choose it on the Home page".to_string()
            },
        ),
        (
            app.game.exe_present,
            format!("Game executable ({exe})"),
            if app.game.exe_present {
                String::new()
            } else {
                "not in your copy yet".to_string()
            },
        ),
        (
            false,
            "Translated shaders".to_string(),
            "arrives in M1.2–M1.8".to_string(),
        ),
        (
            false,
            "Runtime: loader, system shims, D3D12 on Vulkan".to_string(),
            "arrives in M4.0–M4.7".to_string(),
        ),
    ]
}

impl LeonidaApp {
    pub(crate) fn run_page(&mut self, ui: &mut egui::Ui) {
        header(ui, "Run", "Start the game on the Leonida runtime.");
        let reqs = requirements(self);
        let ready = reqs.iter().all(|(met, _, _)| *met);
        card(ui, |ui| {
            card_title(ui, "Before the game can run");
            for (met, what, why) in &reqs {
                check_line(ui, *met, what, why);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled(ready, egui::Button::new(RichText::new("▶  Run").strong()))
                    .on_disabled_hover_text("Everything above must be ✔ first");
                later_button(ui, "▶  Run with trace", "M4.7");
                later_button(ui, "🔍 Check imports (dry run)", "M4.2");
            });
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            card_title(ui, "Boot reports");
            ui.weak("Each run with trace writes what it reached here: imports hit, unknown D3D12 calls, files read, the last address.");
        });
        ui.add_space(8.0);
        planned(ui, "M4–M5", "The runtime and the first boot");
    }

    pub(crate) fn run_dialog(&mut self, ctx: &egui::Context) {
        let reqs = requirements(self);
        let response = egui::Modal::new(egui::Id::unique("run-dialog")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Run game");
            ui.add_space(6.0);
            for (met, what, why) in &reqs {
                check_line(ui, *met, what, why);
            }
            ui.add_space(10.0);
            let ready = reqs.iter().all(|(met, _, _)| *met);
            let mut close = false;
            ui.horizontal(|ui| {
                ui.add_enabled(ready, egui::Button::new("▶  Launch"))
                    .on_disabled_hover_text("Not everything the game needs exists yet");
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            close
        });
        if response.inner || response.should_close() {
            self.show_run_dialog = false;
        }
    }
}
