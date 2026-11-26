use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Sense, Vec2};

use super::{card, card_title, header, later_button, planned};
use crate::app::LeonidaApp;

impl LeonidaApp {
    pub(crate) fn viewer_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Viewer",
            "The game's models drawn with the game's own translated shaders, on Vulkan.",
        );
        ui.horizontal(|ui| {
            later_button(ui, "📂 Open model…", "M3.3");
            later_button(ui, "🗺 Load area…", "M3.6");
            later_button(ui, "⚡ Precompile all pipelines", "M3.4");
        });
        ui.add_space(8.0);
        planned(
            ui,
            "M3.1–M3.6",
            "The Vulkan renderer, scene harness and viewer",
        );
        ui.add_space(8.0);
        ui.columns(2, |columns| {
            let ui = &mut columns[0];
            let size = Vec2::new(ui.available_width(), 360.0);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(12), Color32::from_rgb(8, 9, 12));
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "3D viewport",
                FontId::proportional(18.0),
                Color32::from_gray(110),
            );
            card(&mut columns[1], |ui| {
                card_title(ui, "Graphics");
                let mut scale = 100.0;
                ui.add_enabled(
                    false,
                    egui::Slider::new(&mut scale, 50.0..=200.0)
                        .text("Resolution scale")
                        .suffix("%"),
                );
                let mut upscaler = "Off";
                ui.add_enabled_ui(false, |ui| {
                    egui::ComboBox::from_label("Upscaler")
                        .selected_text(upscaler)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut upscaler, "Off", "Off");
                            ui.selectable_value(&mut upscaler, "FSR 1", "FSR 1");
                        });
                });
                let mut anisotropy = 16.0;
                ui.add_enabled(
                    false,
                    egui::Slider::new(&mut anisotropy, 1.0..=16.0).text("Anisotropic filtering"),
                );
                let mut frame_cap = 60.0;
                ui.add_enabled(
                    false,
                    egui::Slider::new(&mut frame_cap, 30.0..=240.0).text("Frame limit"),
                )
                .on_disabled_hover_text("Needs the game running (M5.3)");
                ui.weak("Saved to Config/graphics.toml (M3.5).");
            });
        });
    }
}
