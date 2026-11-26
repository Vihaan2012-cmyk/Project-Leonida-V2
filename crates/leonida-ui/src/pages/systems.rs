use eframe::egui;

use super::{card, card_title, header, later_button, planned};
use crate::app::LeonidaApp;

const SYSTEMS: &[(&str, &str)] = &[
    ("Vehicles", "Handling, vehicle list, colours, spawn rules"),
    ("Weapons", "Damage, ammo, ranges"),
    ("Peds", "Ped types, catalogues, relationships"),
    (
        "World",
        "Weather and time of day, population zones, fog, lights",
    ),
    ("Audio", "Radio stations, music, sound sets"),
    ("Missions", "Mission tables and story index"),
    ("Text", "Every game string, by language"),
    ("Navigation", "Navmeshes"),
    ("Camera", "Camera settings, first-person"),
];

impl LeonidaApp {
    pub(crate) fn systems_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Systems",
            "How the game behaves, as readable XML and JSON you can edit and override with mods.",
        );
        planned(
            ui,
            "M2.5",
            "Decoding game data into the Systems folder, and converting edits back",
        );
        ui.add_space(8.0);
        egui::Grid::new("systems-grid")
            .num_columns(3)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                for (index, (name, description)) in SYSTEMS.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.set_width(260.0);
                        card(ui, |ui| {
                            card_title(ui, name);
                            ui.weak(*description);
                            ui.add_space(4.0);
                            later_button(ui, "Open", "M2.5");
                        });
                    });
                    if index % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
    }
}
