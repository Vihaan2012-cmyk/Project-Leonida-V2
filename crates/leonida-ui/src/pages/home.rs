use eframe::egui::{self, RichText, Vec2};

use super::{card, card_title, header};
use crate::app::{LeonidaApp, Page};
use crate::build_steps::{STEPS, StepState};
use crate::theme;

const MILESTONES: &[(&str, &str, bool)] = &[
    ("M0", "Foundation: workspace, game detection, CI", true),
    ("M1.1", "Shader containers", true),
    ("M1.2", "RDNA2 decoder and assembler", false),
    ("M1.3", "GPU basics and CPU reference", false),
    ("M1.4–M1.5", "Shader IR and control flow", false),
    ("M1.6–M1.8", "SPIR-V, wave64, shader store", false),
    ("M2", "Assets, systems, mods", false),
    ("M3", "Renderer and viewer", false),
    ("M4", "Runtime: loader, shims, D3D12 on Vulkan", false),
    ("M5", "Boot the game", false),
];

impl LeonidaApp {
    pub(crate) fn home_page(&mut self, ui: &mut egui::Ui) {
        header(
            ui,
            "Welcome to Leonida",
            "GTA VI (Xbox Series 1.0.2.1) on PC, built from your own copy.",
        );

        card(ui, |ui| {
            card_title(ui, "Your copy");
            match (&self.game.version, &self.game.path) {
                (Some(version), Some(path)) => {
                    ui.label(
                        RichText::new(format!("✔ {version}"))
                            .color(theme::GOOD)
                            .strong(),
                    );
                    ui.monospace(path.display().to_string());
                    let exe = self.game.exe_name.clone().unwrap_or_default();
                    if self.game.exe_present {
                        ui.label(format!("Executable {exe} found."));
                    } else {
                        ui.weak(format!("Executable {exe} is not in your copy yet: everything except running can be built."));
                    }
                }
                (None, Some(path)) => {
                    ui.label(
                        RichText::new("✖ This folder is not a supported version").color(theme::BAD),
                    );
                    ui.monospace(path.display().to_string());
                }
                _ => {
                    ui.weak("No game folder chosen yet.");
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("📂 Choose folder…").clicked() {
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
            if !self.candidates.is_empty() {
                ui.add_space(6.0);
                ui.label("More than one copy was found:");
                let mut chosen = None;
                for path in &self.candidates {
                    ui.horizontal(|ui| {
                        if ui.button("Use this").clicked() {
                            chosen = Some(path.clone());
                        }
                        ui.monospace(path.display().to_string());
                    });
                }
                if let Some(path) = chosen {
                    self.set_game(path);
                }
            }
        });
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            let size = Vec2::new(200.0, 48.0);
            let can_build = self.game.is_supported() && !self.jobs.is_busy();
            let build = egui::Button::new(RichText::new("🔨  Build game").size(17.0).strong())
                .min_size(size);
            if ui.add_enabled(can_build, build).clicked() {
                let ctx = ui.ctx().clone();
                self.start_build(&ctx);
            }
            let run = egui::Button::new(RichText::new("▶  Run game").size(17.0).strong())
                .min_size(size)
                .fill(theme::ACCENT.linear_multiply(0.35));
            if ui.add(run).clicked() {
                self.show_run_dialog = true;
            }
        });
        ui.add_space(10.0);

        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                card_title(ui, "Build steps");
                for (step, report) in STEPS.iter().zip(&self.build) {
                    let (icon, color) = match report.state {
                        StepState::Done => ("✔", theme::GOOD),
                        StepState::Failed => ("✖", theme::BAD),
                        StepState::Waiting => ("○", ui.visuals().weak_text_color()),
                        StepState::NotBuiltYet => ("🚧", theme::PENDING),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon).color(color));
                        ui.label(step.name);
                        ui.weak(&report.detail);
                    });
                }
            });
            card(&mut columns[1], |ui| {
                card_title(ui, "Roadmap");
                for (id, name, done) in MILESTONES {
                    ui.horizontal(|ui| {
                        if *done {
                            ui.label(RichText::new("✔").color(theme::GOOD));
                        } else {
                            ui.label(RichText::new("○").weak());
                        }
                        ui.label(RichText::new(*id).monospace());
                        ui.label(*name);
                    });
                }
            });
        });

        if self.summary.is_some() {
            ui.add_space(10.0);
            if ui.link("See the shader scan results →").clicked() {
                self.page = Page::Shaders;
            }
        }
    }
}
