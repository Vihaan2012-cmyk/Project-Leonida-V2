//! The Leonida window: one tab per group of settings, a Play button and the commands it starts.
//!
//! What each field holds and checks lives in `form` and `model`; this file only draws the fields and passes their
//! text through.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use config::Config;
use game_package::Manifest;
use winsafe::{self as w, co, guard::DeleteObjectGuard, gui, prelude::*};

use crate::Error;
use crate::cache;
use crate::checks::{self, World};
use crate::child;
use crate::dlss;
use crate::form::{self, Choice, Env, Field, Kind};
use crate::game::{self, GameState};
use crate::jobs::{Jobs, Outcome, Wake};
use crate::model::{FieldError, Form, Severity, save_changed};
use crate::modlist::{self, ModList};
use crate::play::{self, Paths, PlayOptions};
use crate::settings::Settings;

const WIDTH: i32 = 1100;
const HEIGHT: i32 = 740;
const TIMER: usize = 1;
const ROW: i32 = 26;
const GRAY: (u8, u8, u8) = (110, 110, 110);
const RED: (u8, u8, u8) = (196, 43, 28);
const AMBER: (u8, u8, u8) = (166, 100, 0);
const GREEN: (u8, u8, u8) = (16, 124, 16);

pub fn run() -> Result<(), Error> {
    let _ = w::FreeConsole();
    Window::new().wnd.run_main(None)?;
    Ok(())
}

enum Ctl {
    Check(gui::CheckBox),
    Combo(gui::ComboBox, Vec<Choice>, bool),
    Edit(gui::Edit),
}

struct FieldUi {
    field: Field,
    ctl: Ctl,
    hint: gui::Label,
    browse: Option<gui::Button>,
}

struct Layout {
    label: i32,
    control: i32,
    hint: i32,
}

fn layout_for(columns: usize) -> (i32, Layout) {
    match columns {
        0 | 1 => (
            700,
            Layout {
                label: 150,
                control: 230,
                hint: 300,
            },
        ),
        2 => (
            520,
            Layout {
                label: 150,
                control: 170,
                hint: 190,
            },
        ),
        _ => (
            350,
            Layout {
                label: 110,
                control: 130,
                hint: 100,
            },
        ),
    }
}

struct State {
    root: PathBuf,
    manifests: Vec<Manifest>,
    monitors: usize,
    form: Form,
    jobs: Jobs,
    log: Vec<String>,
    started: Instant,
    prefs: Settings,
    mods: ModList,
    shown_job: String,
    dlss: dlss::Probe,
}

#[derive(Clone)]
struct Window {
    wnd: gui::WindowMain,
    title: gui::Label,
    status: gui::Label,
    play: gui::Button,
    save: gui::Button,
    revert: gui::Button,
    open_config: gui::Button,
    job: gui::Label,
    progress: gui::ProgressBar,
    cancel: gui::Button,
    log: gui::Edit,
    fields: Rc<Vec<FieldUi>>,
    headings: Rc<Vec<gui::Label>>,
    pages: Rc<Vec<gui::TabPage>>,
    game_status: gui::Label,
    find_game: gui::Button,
    shader_line: gui::Label,
    pipeline_line: gui::Label,
    prepare: gui::Button,
    blocklist_line: gui::Label,
    scan: gui::Button,
    trace: gui::CheckBox,
    boot_line: gui::Label,
    open_boot: gui::Button,
    mods_list: gui::ListView<()>,
    mod_up: gui::Button,
    mod_down: gui::Button,
    mod_on: gui::Button,
    mod_off: gui::Button,
    mod_refresh: gui::Button,
    conflicts: gui::Edit,
    state: Rc<RefCell<State>>,
    colors: Rc<RefCell<(HashSet<usize>, HashSet<usize>)>>,
    status_color: Rc<Cell<(u8, u8, u8)>>,
    updating: Rc<Cell<bool>>,
    fonts: Rc<RefCell<Vec<DeleteObjectGuard<w::HFONT>>>>,
}

fn label<P: GuiParent + 'static>(
    parent: &P,
    text: &str,
    pos: (i32, i32),
    size: (i32, i32),
) -> gui::Label {
    gui::Label::new(
        parent,
        gui::LabelOpts {
            text,
            position: gui::dpi(pos.0, pos.1),
            size: gui::dpi(size.0, size.1),
            ..Default::default()
        },
    )
}

fn button<P: GuiParent + 'static>(
    parent: &P,
    text: &str,
    pos: (i32, i32),
    size: (i32, i32),
) -> gui::Button {
    let (width, height) = gui::dpi(size.0, size.1);
    gui::Button::new(
        parent,
        gui::ButtonOpts {
            text,
            position: gui::dpi(pos.0, pos.1),
            width,
            height,
            ..Default::default()
        },
    )
}

fn edit<P: GuiParent + 'static>(
    parent: &P,
    pos: (i32, i32),
    width: i32,
    height: i32,
    multiline: bool,
) -> gui::Edit {
    let (control_style, window_style) = if multiline {
        (
            co::ES::MULTILINE | co::ES::READONLY | co::ES::AUTOVSCROLL,
            co::WS::CHILD | co::WS::VISIBLE | co::WS::VSCROLL | co::WS::TABSTOP,
        )
    } else {
        (
            co::ES::AUTOHSCROLL,
            co::WS::CHILD | co::WS::VISIBLE | co::WS::TABSTOP | co::WS::GROUP,
        )
    };
    gui::Edit::new(
        parent,
        gui::EditOpts {
            position: gui::dpi(pos.0, pos.1),
            width: gui::dpi_x(width),
            height: gui::dpi(0, height).1,
            control_style,
            window_style,
            ..Default::default()
        },
    )
}

fn list<P: GuiParent + 'static>(
    parent: &P,
    pos: (i32, i32),
    size: (i32, i32),
    columns: &[(&str, i32)],
) -> gui::ListView<()> {
    let columns: Vec<(&str, i32)> = columns
        .iter()
        .map(|&(title, width)| (title, gui::dpi_x(width)))
        .collect();
    gui::ListView::<()>::new(
        parent,
        gui::ListViewOpts {
            position: gui::dpi(pos.0, pos.1),
            size: gui::dpi(size.0, size.1),
            columns: &columns,
            control_ex_style: co::LVS_EX::FULLROWSELECT | co::LVS_EX::DOUBLEBUFFER,
            ..Default::default()
        },
    )
}

fn build_field(page: &gui::TabPage, field: &Field, x: i32, y: i32, l: &Layout) -> FieldUi {
    label(page, &field.label, (x, y + 3), (l.label - 6, 20));
    let cx = x + l.label;
    let mut browse = None;
    let mut control_width = l.control;
    let mut hint_x = cx + l.control + 8;
    if matches!(field.kind, Kind::Folder | Kind::File { .. }) {
        control_width -= 30;
        browse = Some(button(page, "...", (cx + control_width + 4, y), (26, 22)));
    }
    let ctl = match &field.kind {
        Kind::Switch { .. } => Ctl::Check(gui::CheckBox::new(
            page,
            gui::CheckBoxOpts {
                text: "",
                position: gui::dpi(cx, y + 2),
                size: gui::dpi(24, 20),
                ..Default::default()
            },
        )),
        Kind::Choice(choices) | Kind::Combo(choices) => {
            let editable = matches!(field.kind, Kind::Combo(_));
            let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
            Ctl::Combo(
                gui::ComboBox::new(
                    page,
                    gui::ComboBoxOpts {
                        position: gui::dpi(cx, y),
                        width: gui::dpi_x(l.control),
                        control_style: if editable {
                            co::CBS::DROPDOWN
                        } else {
                            co::CBS::DROPDOWNLIST
                        },
                        items: &labels,
                        ..Default::default()
                    },
                ),
                choices.clone(),
                editable,
            )
        }
        _ => Ctl::Edit(edit(page, (cx, y), control_width, 22, false)),
    };
    if matches!(ctl, Ctl::Check(_)) {
        hint_x = cx + 30;
    }
    let hint = label(page, &field.help, (hint_x, y + 2), (l.hint + 20, ROW - 2));
    FieldUi {
        field: field.clone(),
        ctl,
        hint,
        browse,
    }
}

fn monitors() -> Vec<String> {
    let mut found = Vec::new();
    let _ = w::HDC::NULL.EnumDisplayMonitors(None, |monitor, _, _| {
        if let Ok(info) = monitor.GetMonitorInfo() {
            found.push(info.szDevice().trim_start_matches("\\\\.\\").to_string());
        }
        true
    });
    found
}

impl Window {
    fn new() -> Window {
        let screens = monitors();
        let env = Env {
            monitors: screens.clone(),
        };
        let pages_data = form::pages(&env);
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let config_dir = root.join(config::DEFAULT_DIR);
        let mut start_log = Vec::new();
        let config = match Config::load(&config_dir) {
            Ok(config) => config,
            Err(error) => {
                start_log.push(format!("Could not read the settings: {error}"));
                Config::default()
            }
        };

        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: "Leonida",
            size: gui::dpi(WIDTH, HEIGHT),
            style: co::WS::CAPTION
                | co::WS::SYSMENU
                | co::WS::CLIPCHILDREN
                | co::WS::BORDER
                | co::WS::VISIBLE
                | co::WS::MINIMIZEBOX,
            class_bg_brush: gui::Brush::Color(co::COLOR::WINDOW),
            ..Default::default()
        });
        let title = label(&wnd, "Leonida", (16, 8), (300, 40));
        let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
        let _ = gui::Label::new(
            &wnd,
            gui::LabelOpts {
                text: &version,
                position: gui::dpi(WIDTH - 200, 22),
                size: gui::dpi(184, 20),
                control_style: co::SS::RIGHT,
                ..Default::default()
            },
        );

        let mut tab_pages = Vec::new();
        let mut fields = Vec::new();
        let mut headings = Vec::new();
        for page in &pages_data {
            let tab_page = gui::TabPage::new(&wnd, gui::TabPageOpts::default());
            let (width, layout) = layout_for(page.columns.len());
            for (column, items) in page.columns.iter().enumerate() {
                let x = 12 + column as i32 * width;
                for (row, field) in items.iter().enumerate() {
                    let y = 10 + row as i32 * ROW;
                    if field.kind == Kind::Heading {
                        headings.push(label(&tab_page, &field.label, (x, y), (width - 20, 22)));
                    } else {
                        fields.push(build_field(&tab_page, field, x, y, &layout));
                    }
                }
            }
            tab_pages.push((page.title, tab_page));
        }
        let page_of = |id: &str| {
            let at = pages_data
                .iter()
                .position(|page| page.id == id)
                .unwrap_or(0);
            tab_pages[at].1.clone()
        };

        let game_page = page_of(form::GAME);
        let game_status = label(&game_page, "", (12, 130), (1000, 22));
        let find_game = button(&game_page, "Look for the game", (12, 158), (170, 28));
        let shader_line = label(&game_page, "", (12, 206), (700, 20));
        let pipeline_line = label(&game_page, "", (12, 228), (700, 20));
        let prepare = button(&game_page, "Prepare shaders", (12, 258), (170, 28));
        label(
            &game_page,
            "Translates every shader and builds the pipelines (leonida pipelines). It takes a long time.",
            (196, 263),
            (800, 20),
        );

        let content_page = page_of(form::CONTENT);
        let blocklist_line = label(&content_page, "", (12, 52), (700, 20));
        let scan = button(&content_page, "Scan", (12, 80), (170, 28));
        label(
            &content_page,
            "Finds what to hide and writes the blocklist (leonida content scan). Only nudity is filtered.",
            (196, 85),
            (800, 20),
        );

        let trace_page = page_of(form::TRACE);
        let trace = gui::CheckBox::new(
            &trace_page,
            gui::CheckBoxOpts {
                text: "Record a boot trace when playing (--trace)",
                position: gui::dpi(12, 14),
                size: gui::dpi(500, 22),
                ..Default::default()
            },
        );
        let boot_line = label(&trace_page, "", (12, 48), (1000, 20));
        let open_boot = button(&trace_page, "Open the boot folder", (12, 76), (190, 28));

        let mods_page = page_of(form::MODS);
        let mods_list = list(
            &mods_page,
            (12, 44),
            (1030, 230),
            &[
                ("Priority", 70),
                ("On", 50),
                ("Package", 220),
                ("Name", 240),
                ("Version", 80),
                ("Type", 80),
                ("Problem", 270),
            ],
        );
        let mod_up = button(&mods_page, "Lower priority", (12, 282), (130, 28));
        let mod_down = button(&mods_page, "Higher priority", (150, 282), (130, 28));
        let mod_on = button(&mods_page, "Turn on", (288, 282), (110, 28));
        let mod_off = button(&mods_page, "Turn off", (406, 282), (110, 28));
        let mod_refresh = button(&mods_page, "Look again", (524, 282), (110, 28));
        label(
            &mods_page,
            "Files more than one package provides (the later in the order wins):",
            (12, 322),
            (800, 20),
        );
        let conflicts = edit(&mods_page, (12, 346), 1030, 90, true);

        let pages_only: Vec<(&str, gui::TabPage)> = tab_pages
            .iter()
            .map(|(title, page)| (*title, page.clone()))
            .collect();
        let _ = gui::Tab::new(
            &wnd,
            gui::TabOpts {
                position: gui::dpi(16, 52),
                size: gui::dpi(WIDTH - 32, 490),
                pages: &pages_only,
                ..Default::default()
            },
        );

        let play = button(&wnd, "Play", (16, 552), (150, 36));
        let save = button(&wnd, "Save settings", (176, 552), (130, 36));
        let revert = button(&wnd, "Revert", (314, 552), (100, 36));
        let open_config = button(&wnd, "Open settings folder", (422, 552), (160, 36));
        let status = label(&wnd, "", (596, 560), (WIDTH - 612, 22));
        let job = label(&wnd, "", (16, 598), (600, 20));
        let progress = gui::ProgressBar::new(
            &wnd,
            gui::ProgressBarOpts {
                position: gui::dpi(630, 600),
                size: gui::dpi(340, 14),
                range: (0, 1000),
                ..Default::default()
            },
        );
        let cancel = button(&wnd, "Cancel", (984, 592), (100, 28));
        let log = edit(&wnd, (16, 628), WIDTH - 32, 100, true);

        let manifests = Manifest::builtin();
        let form = Form::new(&config, &pages_data);
        let mods = ModList::default();
        let window = Window {
            wnd,
            title,
            status,
            play,
            save,
            revert,
            open_config,
            job,
            progress,
            cancel,
            log,
            fields: Rc::new(fields),
            headings: Rc::new(headings),
            pages: Rc::new(tab_pages.into_iter().map(|(_, page)| page).collect()),
            game_status,
            find_game,
            shader_line,
            pipeline_line,
            prepare,
            blocklist_line,
            scan,
            trace,
            boot_line,
            open_boot,
            mods_list,
            mod_up,
            mod_down,
            mod_on,
            mod_off,
            mod_refresh,
            conflicts,
            state: Rc::new(RefCell::new(State {
                root,
                manifests,
                monitors: screens.len(),
                form,
                jobs: Jobs::default(),
                log: start_log,
                started: Instant::now(),
                prefs: Settings::load(),
                mods,
                shown_job: String::new(),
                dlss: dlss::Probe::default(),
            })),
            colors: Rc::new(RefCell::new((HashSet::new(), HashSet::new()))),
            status_color: Rc::new(Cell::new(GREEN)),
            updating: Rc::new(Cell::new(false)),
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
        self.wnd
            .on()
            .wm_ctl_color_static(move |p| me.paint(&p.hdc, &p.hwnd));
        for page in self.pages.iter() {
            let me = self.clone();
            page.on()
                .wm_ctl_color_static(move |p| me.paint(&p.hdc, &p.hwnd));
        }

        for index in 0..self.fields.len() {
            let me = self.clone();
            match &self.fields[index].ctl {
                Ctl::Check(check) => check.on().bn_clicked(move || {
                    me.changed(index);
                    Ok(())
                }),
                Ctl::Combo(combo, _, editable) => {
                    let again = self.clone();
                    combo.on().cbn_sel_change(move || {
                        me.changed(index);
                        Ok(())
                    });
                    if *editable {
                        combo.on().cbn_edit_change(move || {
                            again.changed(index);
                            Ok(())
                        });
                    }
                }
                Ctl::Edit(edit) => edit.on().en_change(move || {
                    me.changed(index);
                    Ok(())
                }),
            }
            if let Some(browse) = &self.fields[index].browse {
                let me = self.clone();
                browse.on().bn_clicked(move || {
                    me.browse(index);
                    Ok(())
                });
            }
        }

        let me = self.clone();
        self.play.on().bn_clicked(move || {
            me.start_playing();
            Ok(())
        });
        let me = self.clone();
        self.save.on().bn_clicked(move || {
            me.save_settings();
            Ok(())
        });
        let me = self.clone();
        self.revert.on().bn_clicked(move || {
            me.revert_settings();
            Ok(())
        });
        let me = self.clone();
        self.open_config.on().bn_clicked(move || {
            let dir = me.paths().config_dir;
            let _ = std::fs::create_dir_all(&dir);
            open_folder(&dir);
            Ok(())
        });
        let me = self.clone();
        self.cancel.on().bn_clicked(move || {
            me.state.borrow().jobs.cancel();
            me.log("Cancelling...");
            Ok(())
        });
        let me = self.clone();
        self.find_game.on().bn_clicked(move || {
            me.start_find();
            Ok(())
        });
        let me = self.clone();
        self.prepare.on().bn_clicked(move || {
            me.start_prepare();
            Ok(())
        });
        let me = self.clone();
        self.scan.on().bn_clicked(move || {
            me.start_scan();
            Ok(())
        });
        let me = self.clone();
        self.trace.on().bn_clicked(move || {
            let prefs = {
                let mut state = me.state.borrow_mut();
                state.prefs.trace = me.trace.is_checked();
                state.prefs.clone()
            };
            if let Err(error) = prefs.save() {
                me.log(&format!("Could not save the trace choice: {error}"));
            }
            Ok(())
        });
        let me = self.clone();
        self.open_boot.on().bn_clicked(move || {
            let dir = me.paths().boot_dir();
            open_folder(if dir.is_dir() {
                &dir
            } else {
                dir.parent().unwrap_or(&dir)
            });
            Ok(())
        });
        for (button, action) in [
            (&self.mod_up, ModAction::Lower),
            (&self.mod_down, ModAction::Higher),
            (&self.mod_on, ModAction::On),
            (&self.mod_off, ModAction::Off),
        ] {
            let me = self.clone();
            button.on().bn_clicked(move || {
                me.mod_action(action);
                Ok(())
            });
        }
        let me = self.clone();
        self.mod_refresh.on().bn_clicked(move || {
            me.scan_mods();
            Ok(())
        });
    }

    fn paint(&self, hdc: &w::HDC, hwnd: &w::HWND) -> w::AnyResult<w::HBRUSH> {
        hdc.SetBkMode(co::BKMODE::TRANSPARENT)?;
        let key = hwnd.ptr() as usize;
        let (errors, notes) = &*self.colors.borrow();
        let (r, g, b) = if hwnd.ptr() == self.status.hwnd().ptr() {
            self.status_color.get()
        } else if errors.contains(&key) {
            RED
        } else if notes.contains(&key) {
            AMBER
        } else if self
            .fields
            .iter()
            .any(|ui| ui.hint.hwnd().ptr() == hwnd.ptr())
        {
            GRAY
        } else {
            (0, 0, 0)
        };
        hdc.SetTextColor(w::COLORREF::from_rgb(r, g, b))?;
        Ok(w::HBRUSH::GetSysColorBrush(co::COLOR::WINDOW)?)
    }

    fn on_create(&self) -> Result<(), Error> {
        let mut fonts = self.fonts.borrow_mut();
        let title = font(30, co::FW::SEMIBOLD)?;
        let status = font(16, co::FW::SEMIBOLD)?;
        let heading = font(16, co::FW::SEMIBOLD)?;
        let small = font(13, co::FW::NORMAL)?;
        set_font(self.title.hwnd(), &title);
        set_font(self.status.hwnd(), &status);
        for label in self.headings.iter() {
            set_font(label.hwnd(), &heading);
        }
        for ui in self.fields.iter() {
            set_font(ui.hint.hwnd(), &small);
        }
        fonts.extend([title, status, heading, small]);
        drop(fonts);
        self.wnd.hwnd().SetTimer(TIMER, 100, None)?;

        let pending = std::mem::take(&mut self.state.borrow_mut().log);
        for line in pending {
            self.log(&line);
        }
        self.trace.set_check(self.state.borrow().prefs.trace);
        self.show_all();
        self.scan_mods();
        let has_game = !self
            .state
            .borrow()
            .form
            .value("general.game_folder")
            .is_empty();
        if !has_game {
            self.start_find();
        }
        Ok(())
    }

    fn paths(&self) -> Paths {
        let state = self.state.borrow();
        let general = state
            .form
            .build()
            .map(|config| config.general)
            .unwrap_or_else(|_| state.form.config().general.clone());
        Paths::of(&state.root, config::DEFAULT_DIR, &general)
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

    fn read(&self, index: usize) -> String {
        let ui = &self.fields[index];
        match &ui.ctl {
            Ctl::Check(check) => {
                let Kind::Switch { on, off } = &ui.field.kind else {
                    return String::new();
                };
                if check.is_checked() { on } else { off }.clone()
            }
            Ctl::Combo(combo, choices, editable) => {
                if *editable {
                    combo.hwnd().GetWindowText().unwrap_or_default()
                } else {
                    combo
                        .items()
                        .selected_index()
                        .and_then(|at| choices.get(at as usize))
                        .map(|choice| choice.value.clone())
                        .unwrap_or_default()
                }
            }
            Ctl::Edit(edit) => edit.text().unwrap_or_default(),
        }
    }

    fn write(&self, index: usize, text: &str) {
        let ui = &self.fields[index];
        self.updating.set(true);
        match &ui.ctl {
            Ctl::Check(check) => {
                let Kind::Switch { on, .. } = &ui.field.kind else {
                    return;
                };
                check.set_check(text == on);
            }
            Ctl::Combo(combo, choices, editable) => {
                if *editable {
                    let _ = combo.hwnd().SetWindowText(text);
                } else {
                    let at = choices.iter().position(|c| c.value == text);
                    combo.items().select(at.map(|at| at as u32));
                }
            }
            Ctl::Edit(edit) => {
                if edit.text().unwrap_or_default() != text {
                    let _ = edit.set_text(text);
                }
            }
        }
        self.updating.set(false);
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.fields.iter().position(|ui| ui.field.id == id)
    }

    fn show_all(&self) {
        for index in 0..self.fields.len() {
            let text = self
                .state
                .borrow()
                .form
                .value(&self.fields[index].field.id)
                .to_string();
            self.write(index, &text);
        }
        self.refresh();
    }

    fn changed(&self, index: usize) {
        if self.updating.get() {
            return;
        }
        let text = self.read(index);
        let id = self.fields[index].field.id.clone();
        let others = self.state.borrow_mut().form.set(&id, &text);
        for other in others {
            if let Some(at) = self.index_of(&other) {
                let text = self.state.borrow().form.value(&other).to_string();
                self.write(at, &text);
            }
        }
        if id == "mods.load_order" {
            self.rebuild_mods(None);
        }
        self.refresh();
    }

    fn browse(&self, index: usize) {
        let ui = &self.fields[index];
        let picked = match &ui.field.kind {
            Kind::Folder => rfd::FileDialog::new()
                .set_title(format!("Choose {}", ui.field.label.to_lowercase()))
                .pick_folder(),
            Kind::File { extension } => rfd::FileDialog::new()
                .set_title(format!("Choose {}", ui.field.label))
                .add_filter(*extension, &[*extension])
                .pick_file(),
            _ => None,
        };
        if let Some(path) = picked {
            let text = path.display().to_string();
            self.write(index, &text);
            self.changed(index);
        }
    }

    /// Checks everything and redraws what depends on it: the hints, the enabled fields, the status line.
    fn refresh(&self) {
        let (errors, dirty, game_line, manifests_ok) = {
            let mut state = self.state.borrow_mut();
            let mut errors: Vec<FieldError> = Vec::new();
            match state.form.build() {
                Ok(config) => {
                    let asks_ngx = config.graphics.upscaler == config::Upscaler::Dlss
                        && config
                            .upscaling
                            .dlss_path
                            .as_ref()
                            .is_none_or(|path| path.is_file());
                    state
                        .dlss
                        .want(asks_ngx.then(|| config.upscaling.dlss_path.clone()));
                    let line = state.dlss.line();
                    let paths = Paths::of(&state.root, config::DEFAULT_DIR, &config.general);
                    errors.extend(checks::check(
                        &config,
                        &World {
                            manifests: &state.manifests,
                            paths: &paths,
                            monitors: state.monitors,
                            dlss: line.as_ref(),
                        },
                    ));
                }
                Err(found) => errors.extend(found),
            }
            (errors, state.form.is_dirty(), (), ())
        };
        let _ = (game_line, manifests_ok);
        let mut by_field = BTreeMap::new();
        let mut general = Vec::new();
        for error in errors {
            match error.id.clone() {
                Some(id) if self.index_of(&id).is_some() => {
                    by_field.entry(id).or_insert(error);
                }
                _ => general.push(error),
            }
        }
        {
            let mut colors = self.colors.borrow_mut();
            colors.0.clear();
            colors.1.clear();
        }
        for ui in self.fields.iter() {
            let enabled = self.state.borrow().form.is_enabled(&ui.field);
            let (text, severity) = match by_field.get(&ui.field.id) {
                Some(error) => (error.message.clone(), Some(error.severity)),
                None => (ui.field.help.clone(), None),
            };
            let key = ui.hint.hwnd().ptr() as usize;
            match severity {
                Some(Severity::Error) => self.colors.borrow_mut().0.insert(key),
                Some(Severity::Note) => self.colors.borrow_mut().1.insert(key),
                Some(Severity::Info) | None => false,
            };
            let _ = ui.hint.hwnd().SetWindowText(&text);
            ui.hint.hwnd().InvalidateRect(None, true).ok();
            match &ui.ctl {
                Ctl::Check(c) => c.hwnd().EnableWindow(enabled),
                Ctl::Combo(c, _, _) => c.hwnd().EnableWindow(enabled),
                Ctl::Edit(c) => c.hwnd().EnableWindow(enabled),
            };
            if let Some(browse) = &ui.browse {
                browse.hwnd().EnableWindow(enabled);
            }
        }
        let blocking = by_field
            .values()
            .chain(general.iter())
            .filter(|e| e.severity == Severity::Error)
            .count();
        let busy = self.state.borrow().jobs.is_busy();
        let (color, text) = if blocking > 0 {
            let first = by_field
                .values()
                .chain(general.iter())
                .find(|e| e.severity == Severity::Error)
                .map(|e| match &e.id {
                    Some(id) => format!("{id}: {}", e.message),
                    None => e.message.clone(),
                })
                .unwrap_or_default();
            (RED, format!("{blocking} problem(s). {first}"))
        } else if dirty {
            (AMBER, "Unsaved changes".to_string())
        } else {
            (GREEN, "Settings saved".to_string())
        };
        self.status_color.set(color);
        let _ = self.status.hwnd().SetWindowText(&text);
        self.save.hwnd().EnableWindow(dirty && blocking == 0);
        self.revert.hwnd().EnableWindow(dirty);
        self.play.hwnd().EnableWindow(blocking == 0 && !busy);
        self.prepare.hwnd().EnableWindow(blocking == 0 && !busy);
        self.scan.hwnd().EnableWindow(blocking == 0 && !busy);
        self.find_game.hwnd().EnableWindow(!busy);
        self.cancel.hwnd().EnableWindow(busy);
        self.refresh_info();
        self.wnd.hwnd().InvalidateRect(None, true).ok();
    }

    fn refresh_info(&self) {
        let paths = self.paths();
        let now = SystemTime::now();
        let made = cache::inspect(&paths.output_dir);
        let game = match &paths.game_dir {
            None => {
                "No game folder chosen. Look for the game, or choose the folder above.".to_string()
            }
            Some(dir) => {
                let state = self.state.borrow();
                match checks::game_state(dir, &state.manifests) {
                    Ok(GameState {
                        version,
                        exe_present,
                        ..
                    }) => format!(
                        "{} in {}{}",
                        version.unwrap_or_default(),
                        dir.display(),
                        if exe_present {
                            ""
                        } else {
                            " (the game's executable is not there)"
                        }
                    ),
                    Err(message) => message,
                }
            }
        };
        let _ = self.game_status.hwnd().SetWindowText(&game);
        let _ = self.shader_line.hwnd().SetWindowText(&format!(
            "{}   ({})",
            made.shader_line(now),
            paths.output_dir.display()
        ));
        let _ = self
            .pipeline_line
            .hwnd()
            .SetWindowText(&made.pipeline_line(now));
        let _ = self.blocklist_line.hwnd().SetWindowText(&format!(
            "{}   ({})",
            made.blocklist_line(now),
            cache::BLOCKLIST
        ));
        let _ = self.boot_line.hwnd().SetWindowText(&format!(
            "Traces are written to {}",
            paths.boot_dir().display()
        ));
    }

    fn save_settings(&self) -> bool {
        let built = self.state.borrow().form.build();
        let config = match built {
            Ok(config) => config,
            Err(errors) => {
                for error in errors {
                    self.log(&format!(
                        "{}: {}",
                        error.id.unwrap_or_default(),
                        error.message
                    ));
                }
                return false;
            }
        };
        let dir = self.paths().config_dir;
        let old = self.state.borrow().form.config().clone();
        match save_changed(&dir, &old, &config) {
            Ok(files) => {
                self.state.borrow_mut().form.reload(&config);
                if !files.is_empty() {
                    self.log(&format!("Saved {} in {}", files.join(", "), dir.display()));
                }
                self.refresh();
                true
            }
            Err(error) => {
                self.log(&format!("Could not save the settings: {error}"));
                false
            }
        }
    }

    fn revert_settings(&self) {
        let dir = self.paths().config_dir;
        match Config::load(&dir) {
            Ok(config) => {
                self.state.borrow_mut().form.reload(&config);
                self.show_all();
                self.rebuild_mods(None);
                self.log("Settings read again from disk");
            }
            Err(error) => self.log(&format!("Could not read the settings: {error}")),
        }
    }

    fn start_playing(&self) {
        if self.state.borrow().form.is_dirty() && !self.save_settings() {
            return;
        }
        let paths = self.paths();
        let options = {
            let state = self.state.borrow();
            PlayOptions {
                trace: state.prefs.trace,
                frame_limit: Some(state.form.value(form::FRAME_LIMIT).to_string()),
            }
        };
        let Ok(program) = std::env::current_exe() else {
            self.log("Could not find the Leonida program to start");
            return;
        };
        let launch = play::play(&program, &paths, &options);
        self.log(&format!("Starting: {}", launch.describe()));
        if let Err(error) = launch.command().spawn() {
            self.log(&format!("Could not start the game: {error}"));
        }
    }

    fn start_command(
        &self,
        name: &'static str,
        build: fn(&std::path::Path, &Paths) -> play::Launch,
    ) {
        if self.state.borrow().form.is_dirty() && !self.save_settings() {
            return;
        }
        let Ok(program) = std::env::current_exe() else {
            self.log("Could not find the Leonida program to start");
            return;
        };
        let launch = build(&program, &self.paths());
        let started = self
            .state
            .borrow_mut()
            .jobs
            .start(name, wake(), move |reporter| {
                Outcome::Command(child::run(&launch, name, reporter))
            });
        if started {
            self.log(&format!("{name} started"));
        }
        self.refresh();
    }

    fn start_prepare(&self) {
        self.start_command("Preparing shaders", play::prepare_shaders);
    }

    fn start_scan(&self) {
        self.start_command("Scanning content", play::scan_content);
    }

    fn start_find(&self) {
        let manifests = self.state.borrow().manifests.clone();
        let started = self.state.borrow_mut().jobs.start(
            "Looking for the game files",
            wake(),
            move |reporter| {
                reporter.busy(
                    "Checking Downloads, Desktop, Documents, Games and the top of each drive",
                );
                Outcome::GamesFound(game::find_games(&manifests))
            },
        );
        if started {
            self.log("Looking for the game files...");
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
            match fraction {
                Some(fraction) => {
                    self.progress.set_marquee(false);
                    self.progress.set_position((fraction * 1000.0) as u32);
                }
                None => self.progress.set_marquee(true),
            }
            let text = format!("{title}... {message}");
            if self.state.borrow().shown_job != text {
                self.state.borrow_mut().shown_job = text.clone();
                self.job.hwnd().SetWindowText(&text)?;
            }
        }
        if self.state.borrow_mut().dlss.poll() {
            self.refresh();
        }
        if let Some(outcome) = polled.finished {
            self.progress.set_marquee(false);
            self.progress.set_position(0);
            self.state.borrow_mut().shown_job.clear();
            self.job.hwnd().SetWindowText("")?;
            self.finish(outcome);
            self.refresh();
        }
        Ok(())
    }

    fn finish(&self, outcome: Outcome) {
        match outcome {
            Outcome::GamesFound(found) => match found.as_slice() {
                [] => self.log("No game files found. Choose the folder on the Game tab."),
                [first, rest @ ..] => {
                    if !rest.is_empty() {
                        self.log(&format!(
                            "Found {} copies of the game files; using the first. Others:",
                            found.len()
                        ));
                        for other in rest {
                            self.log(&format!("  {}", other.display()));
                        }
                    }
                    if let Some(at) = self.index_of("general.game_folder") {
                        let text = first.display().to_string();
                        self.write(at, &text);
                        self.changed(at);
                        self.log(&format!("Using {text}"));
                    }
                }
            },
            Outcome::Command(result) => {
                if result.success {
                    self.log(&format!("{} finished", result.name));
                } else {
                    self.log(&format!("{} failed: {}", result.name, result.detail));
                }
            }
        }
    }

    fn scan_mods(&self) {
        let (root, version) = {
            let state = self.state.borrow();
            (state.root.clone(), game_version(&state.manifests))
        };
        match modlist::inspect(&root, &version, None) {
            Ok(overview) => {
                for note in &overview.notes {
                    self.log(&format!("mods: {note}"));
                }
                let order = load_order(self.state.borrow().form.value("mods.load_order"));
                self.state.borrow_mut().mods = ModList::new(&overview.installed, &order);
                self.draw_mods(None);
                self.update_conflicts();
            }
            Err(error) => self.log(&format!("Could not read the mods: {error}")),
        }
    }

    /// Rebuilds the list from the load order text, after it was typed.
    fn rebuild_mods(&self, _selected: Option<usize>) {
        let order = load_order(self.state.borrow().form.value("mods.load_order"));
        let installed: Vec<modlist::Installed> = self
            .state
            .borrow()
            .mods
            .items()
            .iter()
            .filter(|item| item.problem.as_deref() != Some("not found in Mods"))
            .map(|item| modlist::Installed {
                id: item.id.clone(),
                title: item.title.clone(),
                version: item.version.clone(),
                kind: item.kind.clone(),
                author: String::new(),
                problem: item.problem.clone(),
            })
            .collect();
        self.state.borrow_mut().mods = ModList::new(&installed, &order);
        self.draw_mods(None);
        self.update_conflicts();
    }

    fn draw_mods(&self, select: Option<usize>) {
        let _ = self.mods_list.items().delete_all();
        let items = self.state.borrow().mods.items().to_vec();
        let mut priority = 0;
        for item in &items {
            let place = if item.enabled {
                priority += 1;
                priority.to_string()
            } else {
                String::new()
            };
            let _ = self.mods_list.items().add(
                &[
                    place,
                    if item.enabled { "yes" } else { "" }.to_string(),
                    item.id.clone(),
                    item.title.clone(),
                    item.version.clone(),
                    item.kind.clone(),
                    item.problem.clone().unwrap_or_default(),
                ],
                None,
                (),
            );
        }
        if let Some(at) = select
            && let Some(item) = self.mods_list.items().iter().nth(at)
        {
            let _ = item.select(true);
        }
    }

    fn mod_action(&self, action: ModAction) {
        let Some(index) = self
            .mods_list
            .items()
            .last_selected()
            .map(|item| item.index() as usize)
        else {
            return;
        };
        let (moved, order) = {
            let mut state = self.state.borrow_mut();
            let moved = match action {
                ModAction::Lower => state.mods.move_up(index),
                ModAction::Higher => state.mods.move_down(index),
                ModAction::On => state.mods.set_enabled(index, true),
                ModAction::Off => state.mods.set_enabled(index, false),
            };
            (moved, state.mods.load_order())
        };
        self.draw_mods(moved);
        if let Some(at) = self.index_of("mods.load_order") {
            let text = order.join(", ");
            self.write(at, &text);
            self.state.borrow_mut().form.set("mods.load_order", &text);
        }
        self.update_conflicts();
        self.refresh();
    }

    fn update_conflicts(&self) {
        let (root, version, order) = {
            let state = self.state.borrow();
            (
                state.root.clone(),
                game_version(&state.manifests),
                state.mods.load_order(),
            )
        };
        let text = match modlist::inspect(&root, &version, Some(&order)) {
            Ok(overview) if overview.conflicts.is_empty() => "No contested files.".to_string(),
            Ok(overview) => overview
                .conflicts
                .iter()
                .map(modlist::conflict_line)
                .collect::<Vec<_>>()
                .join("\r\n"),
            Err(error) => error,
        };
        let _ = self.conflicts.set_text(&text);
    }
}

#[derive(Clone, Copy)]
enum ModAction {
    Lower,
    Higher,
    On,
    Off,
}

fn load_order(text: &str) -> Vec<String> {
    text.split([',', ';'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn game_version(manifests: &[Manifest]) -> String {
    manifests
        .last()
        .map(|manifest| manifest.version.clone())
        .unwrap_or_default()
}

fn wake() -> Wake {
    Arc::new(|| {})
}

fn open_folder(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
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
