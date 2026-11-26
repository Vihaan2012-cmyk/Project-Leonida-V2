use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use game_package::Manifest;
use shader_container::{ScanResult, ScanSummary};
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use crate::Error;
use crate::build_steps::{self, STEPS, StepReport, StepState};
use crate::game::{self, GameState};
use crate::jobs::{Jobs, Outcome, Wake};
use crate::settings::Settings;

const WIDTH: i32 = 640;
const HEIGHT: i32 = 890;
const TIMER: usize = 1;
const ORANGE: (u8, u8, u8) = (196, 110, 0);
const GREEN: (u8, u8, u8) = (16, 124, 16);
const RED: (u8, u8, u8) = (196, 43, 28);

pub fn run() -> Result<(), Error> {
    let _ = w::FreeConsole();
    Window::new().wnd.run_main(None)?;
    Ok(())
}

struct State {
    manifests: Vec<Manifest>,
    settings: Settings,
    game: GameState,
    found: Vec<PathBuf>,
    jobs: Jobs,
    steps: Vec<StepReport>,
    scan: Option<ScanResult>,
    log: Vec<String>,
    started: Instant,
    shown_status: String,
}

#[derive(Clone)]
struct Window {
    wnd: gui::WindowMain,
    title: gui::Label,
    status: gui::Label,
    detail: gui::Label,
    primary: gui::Button,
    headings: [gui::Label; 4],
    choose: gui::Button,
    games: gui::ListView<()>,
    use_selected: gui::Button,
    find_again: gui::Button,
    verify: gui::Button,
    open_folder: gui::Button,
    cancel: gui::Button,
    steps: gui::ListView<()>,
    progress: gui::ProgressBar,
    play: gui::Button,
    shader_report: gui::Button,
    mods: gui::Button,
    viewer: gui::Button,
    find_on_start: gui::CheckBox,
    unavailable_options: [gui::CheckBox; 3],
    output_change: gui::Button,
    save_log: gui::Button,
    clear_log: gui::Button,
    log: gui::Edit,
    state: Rc<RefCell<State>>,
    status_color: Rc<Cell<(u8, u8, u8)>>,
    fonts: Rc<RefCell<Vec<DeleteObjectGuard<w::HFONT>>>>,
}

fn label(
    wnd: &gui::WindowMain,
    text: &str,
    pos: (i32, i32),
    size: (i32, i32),
    horz: gui::Horz,
) -> gui::Label {
    gui::Label::new(
        wnd,
        gui::LabelOpts {
            text,
            position: gui::dpi(pos.0, pos.1),
            size: gui::dpi(size.0, size.1),
            resize_behavior: (horz, gui::Vert::None),
            ..Default::default()
        },
    )
}

fn button(
    wnd: &gui::WindowMain,
    text: &str,
    pos: (i32, i32),
    size: (i32, i32),
    horz: gui::Horz,
) -> gui::Button {
    let (width, height) = gui::dpi(size.0, size.1);
    gui::Button::new(
        wnd,
        gui::ButtonOpts {
            text,
            position: gui::dpi(pos.0, pos.1),
            width,
            height,
            resize_behavior: (horz, gui::Vert::None),
            ..Default::default()
        },
    )
}

fn check(wnd: &gui::WindowMain, text: &str, y: i32, checked: bool) -> gui::CheckBox {
    gui::CheckBox::new(
        wnd,
        gui::CheckBoxOpts {
            text,
            position: gui::dpi(16, y),
            size: gui::dpi(WIDTH - 32, 22),
            check_state: if checked {
                co::BST::CHECKED
            } else {
                co::BST::UNCHECKED
            },
            resize_behavior: (gui::Horz::Resize, gui::Vert::None),
            ..Default::default()
        },
    )
}

fn list(wnd: &gui::WindowMain, y: i32, height: i32, columns: &[(&str, i32)]) -> gui::ListView<()> {
    let columns: Vec<(&str, i32)> = columns
        .iter()
        .map(|&(title, width)| (title, gui::dpi_x(width)))
        .collect();
    gui::ListView::<()>::new(
        wnd,
        gui::ListViewOpts {
            position: gui::dpi(16, y),
            size: gui::dpi(WIDTH - 32, height),
            columns: &columns,
            control_ex_style: co::LVS_EX::FULLROWSELECT | co::LVS_EX::DOUBLEBUFFER,
            resize_behavior: (gui::Horz::Resize, gui::Vert::None),
            ..Default::default()
        },
    )
}

fn col(index: i32) -> i32 {
    16 + 154 * index
}

fn local_data_dir() -> String {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("Leonida").display().to_string())
        .unwrap_or_else(|| "Leonida".to_string())
}

impl Window {
    fn new() -> Window {
        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: "Leonida",
            size: gui::dpi(WIDTH, HEIGHT),
            style: co::WS::CAPTION
                | co::WS::SYSMENU
                | co::WS::CLIPCHILDREN
                | co::WS::BORDER
                | co::WS::VISIBLE
                | co::WS::SIZEBOX
                | co::WS::MINIMIZEBOX
                | co::WS::MAXIMIZEBOX,
            class_bg_brush: gui::Brush::Color(co::COLOR::WINDOW),
            ..Default::default()
        });
        let none = gui::Horz::None;
        let stretch = gui::Horz::Resize;
        let right = gui::Horz::Repos;

        let title = label(&wnd, "Leonida", (16, 8), (300, 40), none);
        let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
        let version_label = gui::Label::new(
            &wnd,
            gui::LabelOpts {
                text: &version,
                position: gui::dpi(WIDTH - 200, 22),
                size: gui::dpi(184, 20),
                control_style: co::SS::RIGHT,
                resize_behavior: (right, gui::Vert::None),
                ..Default::default()
            },
        );
        let _ = version_label;
        label(
            &wnd,
            "Plays GTA VI on your PC, using the Xbox Series game files you own.",
            (16, 50),
            (WIDTH - 32, 20),
            stretch,
        );
        let status = label(&wnd, "", (16, 82), (440, 28), stretch);
        let primary = button(&wnd, "Build game", (WIDTH - 160, 80), (144, 34), right);
        let detail = label(&wnd, "", (16, 114), (WIDTH - 32, 20), stretch);

        let game_heading = label(&wnd, "Game files", (16, 150), (300, 22), none);
        let choose = button(
            &wnd,
            "Choose folder...",
            (WIDTH - 160, 144),
            (144, 28),
            right,
        );
        let games = list(
            &wnd,
            176,
            84,
            &[("Where", 330), ("Version", 170), ("Status", 100)],
        );
        let use_selected = button(&wnd, "Use selected", (col(0), 268), (146, 28), none);
        let find_again = button(&wnd, "Look again", (col(1), 268), (146, 28), none);
        let verify = button(&wnd, "Verify files", (col(2), 268), (146, 28), none);
        let open_folder = button(&wnd, "Open folder", (col(3), 268), (146, 28), none);

        let build_heading = label(&wnd, "Build", (16, 314), (300, 22), none);
        let cancel = button(&wnd, "Cancel", (WIDTH - 160, 308), (144, 28), right);
        let steps = list(&wnd, 340, 176, &[("Step", 290), ("Status", 290)]);
        let progress = gui::ProgressBar::new(
            &wnd,
            gui::ProgressBarOpts {
                position: gui::dpi(16, 524),
                size: gui::dpi(WIDTH - 32, 12),
                range: (0, 1000),
                resize_behavior: (stretch, gui::Vert::None),
                ..Default::default()
            },
        );
        let play = button(&wnd, "Play game", (col(0), 544), (146, 28), none);
        let shader_report = button(&wnd, "Shader report...", (col(1), 544), (146, 28), none);
        let mods = button(&wnd, "Mods...", (col(2), 544), (146, 28), none);
        let viewer = button(&wnd, "Model viewer...", (col(3), 544), (146, 28), none);

        let options_heading = label(&wnd, "Options", (16, 588), (300, 22), none);
        let find_on_start = check(
            &wnd,
            "Look for the game files when Leonida opens",
            614,
            true,
        );
        let unavailable_options = [
            check(
                &wnd,
                "Keep built files on disk, so the game starts faster next time",
                638,
                true,
            ),
            check(
                &wnd,
                "Upscale with AMD FSR 1 for higher frame rates",
                662,
                false,
            ),
            check(&wnd, "Unlock the frame rate", 686, false),
        ];
        label(&wnd, "Built files", (16, 720), (90, 20), none);
        let _ = gui::Edit::new(
            &wnd,
            gui::EditOpts {
                text: &local_data_dir(),
                position: gui::dpi(110, 716),
                width: gui::dpi_x(WIDTH - 236),
                height: gui::dpi(0, 24).1,
                control_style: co::ES::AUTOHSCROLL | co::ES::READONLY,
                resize_behavior: (stretch, gui::Vert::None),
                ..Default::default()
            },
        );
        let output_change = button(&wnd, "Change...", (WIDTH - 116, 714), (100, 28), right);

        let activity_heading = label(&wnd, "Activity", (16, 758), (300, 22), none);
        let save_log = button(&wnd, "Save log...", (WIDTH - 264, 752), (120, 28), right);
        let clear_log = button(&wnd, "Clear", (WIDTH - 136, 752), (120, 28), right);
        let log = gui::Edit::new(
            &wnd,
            gui::EditOpts {
                position: gui::dpi(16, 786),
                width: gui::dpi_x(WIDTH - 32),
                height: gui::dpi(0, HEIGHT - 798).1,
                control_style: co::ES::MULTILINE | co::ES::READONLY | co::ES::AUTOVSCROLL,
                window_style: co::WS::CHILD | co::WS::VISIBLE | co::WS::VSCROLL | co::WS::TABSTOP,
                resize_behavior: (stretch, gui::Vert::Resize),
                ..Default::default()
            },
        );

        let settings = Settings::load();
        find_on_start.set_check(settings.find_on_start);
        let window = Window {
            wnd,
            title,
            status,
            detail,
            primary,
            headings: [
                game_heading,
                build_heading,
                options_heading,
                activity_heading,
            ],
            choose,
            games,
            use_selected,
            find_again,
            verify,
            open_folder,
            cancel,
            steps,
            progress,
            play,
            shader_report,
            mods,
            viewer,
            find_on_start,
            unavailable_options,
            output_change,
            save_log,
            clear_log,
            log,
            state: Rc::new(RefCell::new(State {
                manifests: Manifest::builtin(),
                settings,
                game: GameState::default(),
                found: Vec::new(),
                jobs: Jobs::default(),
                steps: build_steps::initial_reports(),
                scan: None,
                log: Vec::new(),
                started: Instant::now(),
                shown_status: String::new(),
            })),
            status_color: Rc::new(Cell::new(GREEN)),
            fonts: Rc::new(RefCell::new(Vec::new())),
        };
        window.events();
        window
    }

    fn events(&self) {
        let me = self.clone();
        self.wnd.on().wm_create(move |_| {
            me.on_create()?;
            Ok(0)
        });
        let me = self.clone();
        self.wnd.on().wm_timer(TIMER, move || me.tick());
        let me = self.clone();
        self.wnd.on().wm_ctl_color_static(move |p| {
            p.hdc.SetBkMode(co::BKMODE::TRANSPARENT)?;
            if p.hwnd.ptr() == me.status.hwnd().ptr() {
                let (r, g, b) = me.status_color.get();
                p.hdc.SetTextColor(w::COLORREF::from_rgb(r, g, b))?;
            }
            Ok(w::HBRUSH::GetSysColorBrush(co::COLOR::WINDOW)?)
        });

        let me = self.clone();
        self.primary.on().bn_clicked(move || {
            me.start_build();
            Ok(())
        });
        let me = self.clone();
        self.choose.on().bn_clicked(move || {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose the folder that contains MicrosoftGame.Config")
                .pick_folder()
            {
                me.set_game(path);
            }
            Ok(())
        });
        let me = self.clone();
        self.use_selected.on().bn_clicked(move || {
            let index = me
                .games
                .items()
                .last_selected()
                .map(|item| item.index() as usize);
            let path = index.and_then(|index| me.state.borrow().found.get(index).cloned());
            if let Some(path) = path {
                me.set_game(path);
            }
            Ok(())
        });
        let me = self.clone();
        self.find_again.on().bn_clicked(move || {
            me.start_find();
            Ok(())
        });
        let me = self.clone();
        self.open_folder.on().bn_clicked(move || {
            if let Some(path) = me.state.borrow().game.path.clone() {
                let _ = std::process::Command::new("explorer").arg(path).spawn();
            }
            Ok(())
        });
        let me = self.clone();
        self.cancel.on().bn_clicked(move || {
            me.state.borrow().jobs.cancel();
            me.log("Cancelling...");
            Ok(())
        });
        let me = self.clone();
        self.shader_report.on().bn_clicked(move || {
            let report = me
                .state
                .borrow()
                .scan
                .as_ref()
                .map(|scan| ScanSummary::of(scan).report());
            let target = rfd::FileDialog::new()
                .set_title("Save shader report")
                .set_file_name("shader-report.txt")
                .save_file();
            if let (Some(report), Some(target)) = (report, target) {
                match std::fs::write(&target, report) {
                    Ok(()) => me.log(&format!("Shader report saved to {}", target.display())),
                    Err(error) => me.log(&format!("Could not save the shader report: {error}")),
                }
            }
            Ok(())
        });
        let me = self.clone();
        self.find_on_start.on().bn_clicked(move || {
            let settings = {
                let mut state = me.state.borrow_mut();
                state.settings.find_on_start = me.find_on_start.is_checked();
                state.settings.clone()
            };
            me.save_settings(&settings);
            Ok(())
        });
        let me = self.clone();
        self.save_log.on().bn_clicked(move || {
            let text = me.state.borrow().log.join("\r\n");
            let target = rfd::FileDialog::new()
                .set_title("Save log")
                .set_file_name("leonida-log.txt")
                .save_file();
            if let Some(target) = target
                && let Err(error) = std::fs::write(&target, text)
            {
                me.log(&format!("Could not save the log: {error}"));
            }
            Ok(())
        });
        let me = self.clone();
        self.clear_log.on().bn_clicked(move || {
            me.state.borrow_mut().log.clear();
            me.log.set_text("")?;
            Ok(())
        });
    }

    fn on_create(&self) -> Result<(), Error> {
        let mut fonts = self.fonts.borrow_mut();
        let title = font(30, co::FW::SEMIBOLD)?;
        let status = font(18, co::FW::SEMIBOLD)?;
        let heading = font(16, co::FW::SEMIBOLD)?;
        set_font(self.title.hwnd(), &title);
        set_font(self.status.hwnd(), &status);
        for label in &self.headings {
            set_font(label.hwnd(), &heading);
        }
        fonts.extend([title, status, heading]);
        drop(fonts);

        for hwnd in [
            self.verify.hwnd(),
            self.play.hwnd(),
            self.mods.hwnd(),
            self.viewer.hwnd(),
            self.output_change.hwnd(),
        ] {
            let _ = hwnd.EnableWindow(false);
        }
        for option in &self.unavailable_options {
            let _ = option.hwnd().EnableWindow(false);
        }
        self.wnd.hwnd().SetTimer(TIMER, 100, None)?;

        let (saved, find) = {
            let state = self.state.borrow();
            (state.settings.game.clone(), state.settings.find_on_start)
        };
        match saved {
            Some(path) => self.set_game(path),
            None if find => self.start_find(),
            None => self.refresh(),
        }
        Ok(())
    }

    fn log(&self, line: &str) {
        let stamped = {
            let mut state = self.state.borrow_mut();
            let secs = state.started.elapsed().as_secs();
            let stamped = format!("{:02}:{:02}  {line}", secs / 60, secs % 60);
            state.log.push(stamped.clone());
            stamped
        };
        self.log.set_selection(i32::MAX, i32::MAX);
        self.log.replace_selection(&format!("{stamped}\r\n"));
    }

    fn save_settings(&self, settings: &Settings) {
        if let Err(error) = settings.save() {
            self.log(&format!("Could not save settings: {error}"));
        }
    }

    fn set_game(&self, path: PathBuf) {
        let (message, settings) = {
            let mut state = self.state.borrow_mut();
            state.game = GameState::recognise(path.clone(), &state.manifests);
            if !state.found.contains(&path) {
                state.found.insert(0, path.clone());
            }
            state.steps = build_steps::initial_reports();
            state.scan = None;
            state.settings.game = Some(path.clone());
            let message = match &state.game.version {
                Some(version) => format!("Using {version} in {}", path.display()),
                None => format!("{} is not a supported version of the game", path.display()),
            };
            (message, state.settings.clone())
        };
        self.save_settings(&settings);
        self.log(&message);
        self.refresh();
    }

    fn wake() -> Wake {
        Arc::new(|| {})
    }

    fn start_find(&self) {
        let started = {
            let mut state = self.state.borrow_mut();
            let manifests = state.manifests.clone();
            state.jobs.start(
                "Looking for the game files",
                Self::wake(),
                move |reporter| {
                    reporter.progress(
                        0.1,
                        "Checking Downloads, Desktop, Documents, Games and the top of each drive",
                    );
                    Outcome::GamesFound(game::find_games(&manifests))
                },
            )
        };
        if started {
            self.log("Looking for the game files...");
        }
        self.refresh();
    }

    fn start_build(&self) {
        let started = {
            let mut state = self.state.borrow_mut();
            let game = state.game.path.clone();
            let manifests = state.manifests.clone();
            state.steps = build_steps::initial_reports();
            state.jobs.start("Building", Self::wake(), move |reporter| {
                build_steps::run_build(game, manifests, reporter)
            })
        };
        if started {
            self.log("Build started");
        }
        self.refresh();
    }

    fn tick(&self) -> Result<(), Error> {
        let (polled, running) = {
            let mut state = self.state.borrow_mut();
            let polled = state.jobs.poll();
            let running = state
                .jobs
                .running()
                .map(|job| (job.title.clone(), job.fraction, job.message.clone()));
            (polled, running)
        };
        for line in &polled.logs {
            self.log(line);
        }
        if let Some((title, fraction, message)) = running {
            self.progress.set_position((fraction * 1000.0) as u32);
            self.show_status(ORANGE, &format!("{title}..."), &message)?;
        }
        if let Some(outcome) = polled.finished {
            self.finish(outcome);
            self.refresh();
        }
        Ok(())
    }

    fn finish(&self, outcome: Outcome) {
        match outcome {
            Outcome::GamesFound(found) => {
                let count = found.len();
                {
                    let mut state = self.state.borrow_mut();
                    for path in found.iter().rev() {
                        if !state.found.contains(path) {
                            state.found.insert(0, path.clone());
                        }
                    }
                }
                match count {
                    0 => self
                        .log("No game files found. Use Choose folder... to point Leonida at them."),
                    1 => self.set_game(found[0].clone()),
                    n => self.log(&format!(
                        "Found {n} copies of the game files. Pick one and press Use selected."
                    )),
                }
            }
            Outcome::Build { steps, scan } => {
                {
                    let mut state = self.state.borrow_mut();
                    state.steps = steps;
                    if scan.is_some() {
                        state.scan = scan;
                    }
                }
                self.log("Build finished");
            }
        }
    }

    fn show_status(&self, color: (u8, u8, u8), status: &str, detail: &str) -> Result<(), Error> {
        let key = format!("{status}|{detail}");
        if self.state.borrow().shown_status == key {
            return Ok(());
        }
        self.state.borrow_mut().shown_status = key;
        self.status_color.set(color);
        self.status
            .hwnd()
            .SetWindowText(&format!("\u{25CF}  {status}"))?;
        self.detail.hwnd().SetWindowText(detail)?;
        Ok(())
    }

    fn refresh(&self) {
        let _ = self.try_refresh();
    }

    fn try_refresh(&self) -> Result<(), Error> {
        let state = self.state.borrow();
        let busy = state.jobs.is_busy();
        let supported = state.game.is_supported();
        let built = STEPS
            .iter()
            .zip(&state.steps)
            .filter(|(step, _)| step.available)
            .all(|(_, report)| report.state == StepState::Done);

        self.games.items().delete_all()?;
        for path in &state.found {
            let recognised = GameState::recognise(path.clone(), &state.manifests);
            let version = recognised
                .version
                .unwrap_or_else(|| "Not supported".to_string());
            let status = if state.game.path.as_ref() == Some(path) {
                "In use"
            } else {
                ""
            };
            self.games.items().add(
                &[path.display().to_string(), version, status.to_string()],
                None,
                (),
            )?;
        }

        self.steps.items().delete_all()?;
        for report in &state.steps {
            let status = match report.state {
                StepState::Done => format!("Done: {}", report.detail),
                StepState::Failed => format!("Failed: {}", report.detail),
                StepState::Waiting if report.detail.is_empty() => "Waiting".to_string(),
                StepState::Waiting => report.detail.clone(),
                StepState::NotBuiltYet => "Not available yet".to_string(),
            };
            self.steps
                .items()
                .add(&[report.name.to_string(), status], None, ())?;
        }

        self.primary.hwnd().EnableWindow(supported && !busy);
        self.choose.hwnd().EnableWindow(!busy);
        self.find_again.hwnd().EnableWindow(!busy);
        self.use_selected
            .hwnd()
            .EnableWindow(!busy && !state.found.is_empty());
        self.open_folder
            .hwnd()
            .EnableWindow(state.game.path.is_some());
        let cancelling = state.jobs.running().is_some_and(|job| job.cancelling());
        self.cancel.hwnd().EnableWindow(busy && !cancelling);
        self.shader_report
            .hwnd()
            .EnableWindow(!busy && state.scan.is_some());

        if busy {
            return Ok(());
        }
        self.progress.set_position(if built { 1000 } else { 0 });
        let (color, status, detail) = match (&state.game.path, &state.game.version) {
            (None, _) => (
                RED,
                "Game files not found".to_string(),
                "Choose the folder that contains MicrosoftGame.Config, or press Look again."
                    .to_string(),
            ),
            (Some(path), None) => (
                RED,
                "These game files aren't supported".to_string(),
                format!("{} isn't the Xbox Series version 1.0.2.1.", path.display()),
            ),
            (Some(_), Some(_)) if built => (
                GREEN,
                "Built".to_string(),
                "Everything that can be prepared is ready. Playing isn't available yet."
                    .to_string(),
            ),
            (Some(path), Some(version)) => (
                GREEN,
                "Ready to build".to_string(),
                format!("{version} in {}", path.display()),
            ),
        };
        drop(state);
        self.show_status(color, &status, &detail)
    }
}

fn font(height: i32, weight: co::FW) -> Result<DeleteObjectGuard<w::HFONT>, Error> {
    let mut logfont = w::LOGFONT::new_face(-gui::dpi(0, height).1, "Segoe UI");
    logfont.lfWeight = weight;
    logfont.lfQuality = co::QUALITY::CLEARTYPE;
    Ok(w::HFONT::CreateFontIndirect(&logfont)?)
}

#[allow(unsafe_code)]
fn set_font(hwnd: &w::HWND, font: &w::HFONT) {
    // SAFETY: WM_SETFONT only stores the handle; Window keeps every font alive for the window's lifetime.
    unsafe {
        hwnd.SendMessage(w::msg::WmSetFont {
            hfont: font.raw_copy(),
            redraw: true,
        });
    }
}
