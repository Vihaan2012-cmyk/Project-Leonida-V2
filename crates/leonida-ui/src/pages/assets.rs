use eframe::egui::{self, RichText};

use super::{card, card_title, header, later_button, planned};
use crate::app::LeonidaApp;

const CATEGORIES: &[(&str, &str)] = &[
    ("👤", "Characters"),
    ("🚗", "Vehicles"),
    ("🏙", "World"),
    ("🪑", "Props"),
    ("🔊", "Audio"),
    ("✨", "Shaders"),
];

impl LeonidaApp {
    pub(crate) fn assets_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Assets",
            "Models, textures and audio, read straight from your archives. Export any of it to glTF, DDS and Ogg.",
        );
        ui.horizontal(|ui| {
            later_button(ui, "🗂 Index archives", "M2.1");
            later_button(ui, "💾 Export selected…", "M2.6");
            later_button(ui, "📁 Open export folder", "M2.6");
        });
        ui.add_space(8.0);
        planned(
            ui,
            "M2.1–M2.4",
            "Indexing archives, decoding textures, models and audio",
        );
        ui.add_space(8.0);
        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                card_title(ui, "Browse");
                for (icon, name) in CATEGORIES {
                    ui.add_enabled(
                        false,
                        egui::Button::selectable(false, format!("{icon}  {name}")),
                    );
                }
            });
            card(&mut columns[1], |ui| {
                card_title(ui, "Preview");
                ui.add_space(60.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("🖼").size(40.0).weak());
                    ui.weak("Textures and models appear here once assets are indexed.");
                });
                ui.add_space(60.0);
            });
        });
    }
}
