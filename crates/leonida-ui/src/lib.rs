mod app;
mod build_steps;
mod game;
mod jobs;
mod pages;
mod theme;

use eframe::egui;

pub use app::LeonidaApp;

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Leonida")
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Leonida",
        options,
        Box::new(|cc| Ok(Box::new(LeonidaApp::new(cc)))),
    )
}
