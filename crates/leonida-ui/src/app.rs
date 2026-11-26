use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Align, Layout, RichText};
use game_package::Manifest;
use serde::{Deserialize, Serialize};
use shader_container::{ScanResult, ScanSummary};

use crate::build_steps::{self, StepReport};
use crate::game::{self, GameState};
use crate::jobs::{Jobs, Outcome, Wake};
use crate::pages::shaders::ShaderView;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Page {
    #[default]
    Home,
    Shaders,
    Assets,
    Systems,
    Mods,
    Viewer,
    Run,
    Logs,
    Settings,
}

impl Page {
    const MAIN: [(Page, &'static str, &'static str); 7] = [
        (Page::Home, "🏠", "Home"),
        (Page::Shaders, "✨", "Shaders"),
        (Page::Assets, "📦", "Assets"),
        (Page::Systems, "⚙", "Systems"),
        (Page::Mods, "🧩", "Mods"),
        (Page::Viewer, "🌆", "Viewer"),
        (Page::Run, "▶", "Run"),
    ];
    const FOOTER: [(Page, &'static str, &'static str); 2] = [
        (Page::Logs, "📜", "Logs"),
        (Page::Settings, "🔧", "Settings"),
    ];
}

#[derive(Serialize, Deserialize)]
struct Saved {
    page: Page,
    game: Option<PathBuf>,
    dark: bool,
}

pub struct LeonidaApp {
    pub(crate) page: Page,
    pub(crate) dark: bool,
    pub(crate) manifests: Vec<Manifest>,
    pub(crate) game: GameState,
    pub(crate) candidates: Vec<PathBuf>,
    pub(crate) jobs: Jobs,
    pub(crate) scan: Option<ScanResult>,
    pub(crate) summary: Option<ScanSummary>,
    pub(crate) shader_view: ShaderView,
    pub(crate) build: Vec<StepReport>,
    pub(crate) logs: Vec<String>,
    pub(crate) show_run_dialog: bool,
    started: Instant,
}

impl LeonidaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let saved: Option<Saved> = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, eframe::APP_KEY));
        let dark = saved.as_ref().is_none_or(|saved| saved.dark);
        theme::install(&cc.egui_ctx, dark);
        let manifests = Manifest::builtin();
        let mut app = LeonidaApp {
            page: saved.as_ref().map(|saved| saved.page).unwrap_or_default(),
            dark,
            game: GameState::default(),
            candidates: Vec::new(),
            jobs: Jobs::default(),
            scan: None,
            summary: None,
            shader_view: ShaderView::default(),
            build: build_steps::initial_reports(),
            logs: Vec::new(),
            show_run_dialog: false,
            started: Instant::now(),
            manifests,
        };
        match saved.and_then(|saved| saved.game) {
            Some(path) => app.set_game(path),
            None => app.find_game(&cc.egui_ctx),
        }
        app
    }

    pub(crate) fn log(&mut self, message: impl Into<String>) {
        let elapsed = self.started.elapsed().as_secs();
        self.logs.push(format!(
            "[{:02}:{:02}] {}",
            elapsed / 60,
            elapsed % 60,
            message.into()
        ));
    }

    fn wake(ctx: &egui::Context) -> Wake {
        let ctx = ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

    pub(crate) fn set_game(&mut self, path: PathBuf) {
        self.game = GameState::recognise(path, &self.manifests);
        self.candidates.clear();
        self.scan = None;
        self.summary = None;
        self.shader_view = ShaderView::default();
        self.build = build_steps::initial_reports();
        let path = self
            .game
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        match &self.game.version {
            Some(version) => self.log(format!("Game: {version} at {path}")),
            None => self.log(format!(
                "{path} is not a supported version (needs Xbox Series 1.0.2.1)"
            )),
        }
    }

    pub(crate) fn pick_game_folder(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Choose your game folder (the one with MicrosoftGame.Config)")
            .pick_folder();
        if let Some(path) = picked {
            self.set_game(path);
        }
    }

    pub(crate) fn find_game(&mut self, ctx: &egui::Context) {
        let manifests = self.manifests.clone();
        if self
            .jobs
            .start("Looking for your copy", Self::wake(ctx), move |reporter| {
                reporter.progress(0.0, "Downloads, Desktop, Documents, Games and drive roots");
                Outcome::GamesFound(game::find_games(&manifests))
            })
        {
            self.log("Looking for your copy of the game");
        }
    }

    pub(crate) fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(path) = self.game.path.clone() else {
            return;
        };
        if self
            .jobs
            .start("Scanning shaders", Self::wake(ctx), move |reporter| {
                Outcome::ShaderScan(build_steps::scan_shaders(&path, reporter, 0.0, 1.0))
            })
        {
            self.log("Scanning shaders.rpf");
        }
    }

    pub(crate) fn start_build(&mut self, ctx: &egui::Context) {
        let game = self.game.path.clone();
        let manifests = self.manifests.clone();
        if self
            .jobs
            .start("Building", Self::wake(ctx), move |reporter| {
                build_steps::run_build(game, manifests, reporter)
            })
        {
            self.build = build_steps::initial_reports();
            self.log("Build started");
        }
    }

    fn take_scan(&mut self, result: ScanResult) {
        let summary = ScanSummary::of(&result);
        self.log(format!(
            "Shader scan: {} containers, {} rejected candidates",
            summary.carved, summary.rejected
        ));
        self.summary = Some(summary);
        self.scan = Some(result);
        self.shader_view = ShaderView::default();
    }

    fn poll_jobs(&mut self) {
        let polled = self.jobs.poll();
        for line in polled.logs {
            self.log(line);
        }
        match polled.finished {
            None => {}
            Some(Outcome::GamesFound(found)) => match found.as_slice() {
                [] => {
                    self.log("No supported copy found automatically. Choose the folder yourself.")
                }
                [only] => self.set_game(only.clone()),
                many => {
                    self.log(format!(
                        "Found {} copies. Pick one on the Home page.",
                        many.len()
                    ));
                    self.candidates = found;
                }
            },
            Some(Outcome::ShaderScan(Ok(result))) => self.take_scan(result),
            Some(Outcome::ShaderScan(Err(error))) => {
                self.log(format!("Shader scan stopped: {error}"))
            }
            Some(Outcome::Build { steps, scan }) => {
                self.build = steps;
                if let Some(result) = scan {
                    self.take_scan(result);
                }
                self.log("Build finished");
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new("◆ Leonida")
                    .size(20.0)
                    .strong()
                    .color(theme::ACCENT),
            );
            ui.add_space(12.0);
            match (&self.game.version, &self.game.path) {
                (Some(version), Some(path)) => {
                    ui.label(RichText::new(format!("✔ {version}")).color(theme::GOOD));
                    ui.weak(game::short_path(path, 48));
                }
                (None, Some(path)) => {
                    ui.label(RichText::new("✖ Unsupported folder").color(theme::BAD));
                    ui.weak(game::short_path(path, 48));
                }
                _ => {
                    ui.weak("No game folder chosen");
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.dark { "☀" } else { "🌙" };
                if ui
                    .button(label)
                    .on_hover_text("Switch light/dark")
                    .clicked()
                {
                    self.dark = !self.dark;
                    theme::set_dark(ui.ctx(), self.dark);
                }
                if ui.button("▶ Run game").clicked() {
                    self.show_run_dialog = true;
                }
                let can_build = self.game.is_supported() && !self.jobs.is_busy();
                if ui
                    .add_enabled(can_build, egui::Button::new("🔨 Build game"))
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.start_build(&ctx);
                }
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
            for (page, icon, label) in Page::MAIN {
                self.nav_item(ui, page, icon, label);
            }
            ui.add_space(8.0);
            ui.separator();
            for (page, icon, label) in Page::FOOTER {
                self.nav_item(ui, page, icon, label);
            }
        });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, page: Page, icon: &str, label: &str) {
        let text = RichText::new(format!("{icon}  {label}")).size(15.0);
        if ui.selectable_label(self.page == page, text).clicked() {
            self.page = page;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| match self.jobs.running() {
            Some(running) => {
                ui.spinner();
                ui.strong(&running.title);
                ui.add(
                    egui::ProgressBar::new(running.fraction)
                        .desired_width(240.0)
                        .show_percentage(),
                );
                ui.weak(&running.message);
                let label = if running.cancelling() {
                    "Cancelling…"
                } else {
                    "Cancel"
                };
                if ui
                    .add_enabled(!running.cancelling(), egui::Button::new(label))
                    .clicked()
                {
                    self.jobs.cancel();
                }
            }
            None => {
                ui.label(RichText::new("●").color(theme::GOOD));
                ui.weak(self.logs.last().map(String::as_str).unwrap_or("Ready"));
            }
        });
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        match self.page {
            Page::Shaders => self.shaders_page(ui),
            page => {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| match page {
                        Page::Home => self.home_page(ui),
                        Page::Assets => self.assets_page(ui),
                        Page::Systems => self.systems_page(ui),
                        Page::Mods => self.mods_page(ui),
                        Page::Viewer => self.viewer_page(ui),
                        Page::Run => self.run_page(ui),
                        Page::Logs => self.logs_page(ui),
                        Page::Settings => self.settings_page(ui),
                        Page::Shaders => {}
                    });
            }
        }
    }
}

impl eframe::App for LeonidaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_jobs();
        egui::Panel::top("top-bar")
            .exact_size(48.0)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status-bar")
            .exact_size(32.0)
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(190.0)
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.page(ui));
        if self.show_run_dialog {
            self.run_dialog(ui.ctx());
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let saved = Saved {
            page: self.page,
            game: self.game.path.clone(),
            dark: self.dark,
        };
        eframe::set_value(storage, eframe::APP_KEY, &saved);
    }
}
