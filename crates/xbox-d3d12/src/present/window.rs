//! The window the frames are shown in: a `winit` event loop on its own thread and the state it reports.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use super::{NativeWindow, PresentError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowMode {
    #[default]
    Windowed,
    /// A borderless window over the whole monitor; the swap chain still flips, so there is no mode switch to wait for.
    Borderless,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    /// Client area in physical pixels.
    pub size: (u32, u32),
    pub minimized: bool,
    pub focused: bool,
    pub close_requested: bool,
    pub mode: WindowMode,
    pub scale_factor: f64,
    /// Counts size changes, so a presenter can tell a new size from the same one.
    pub resizes: u64,
    /// The cursor is locked to the window and hidden, and raw mouse motion is going to the motion sink.
    pub cursor_captured: bool,
}

impl WindowState {
    pub fn new(size: (u32, u32), mode: WindowMode) -> WindowState {
        WindowState {
            size,
            minimized: size.0 == 0 || size.1 == 0,
            focused: true,
            close_requested: false,
            mode,
            scale_factor: 1.0,
            resizes: 0,
            cursor_captured: false,
        }
    }

    /// A window with no client area is minimised; its last real size is kept for when it comes back.
    pub fn resized(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.minimized = true;
        } else {
            self.minimized = false;
            if self.size != (width, height) {
                self.size = (width, height);
                self.resizes += 1;
            }
        }
    }
}

pub type EventSink = Box<dyn Fn(&WindowEvent) + Send + Sync>;

/// Receives raw mouse movement in counts (right and down are positive), from the mouse itself and not from the
/// cursor, so it keeps coming at the edge of the screen. Only sent while the cursor is captured.
pub type MotionSink = Box<dyn Fn(i32, i32) + Send + Sync>;

/// Carries the fraction of a count that a whole-count sink cannot take to the next movement.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MotionAccumulator {
    remainder: (f64, f64),
}

impl MotionAccumulator {
    pub fn add(&mut self, dx: f64, dy: f64) -> (i32, i32) {
        let x = self.remainder.0 + dx;
        let y = self.remainder.1 + dy;
        let (wx, wy) = (x.trunc(), y.trunc());
        self.remainder = (x - wx, y - wy);
        (wx as i32, wy as i32)
    }

    pub fn clear(&mut self) {
        self.remainder = (0.0, 0.0);
    }
}

/// What the capture rule says to do about a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// The game gets it.
    Forward,
    /// It only brings the cursor back under the game's control; the game does not see it.
    Swallow { capture: bool },
}

/// When the cursor is locked to the window and hidden: while the window has the keyboard focus, until the player
/// gives it back to the system with Escape (when that is enabled) or switches away with Alt+Tab, which takes the
/// focus. A click in the window takes it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capture {
    focused: bool,
    captured: bool,
    given_back: bool,
    swallow_release: bool,
    pub release_on_escape: bool,
}

impl Capture {
    pub fn new(release_on_escape: bool) -> Capture {
        Capture {
            focused: false,
            captured: false,
            given_back: false,
            swallow_release: false,
            release_on_escape,
        }
    }

    pub fn captured(&self) -> bool {
        self.captured
    }

    fn set(&mut self, captured: bool) -> Option<bool> {
        (self.captured != captured).then(|| {
            self.captured = captured;
            captured
        })
    }

    /// Returns the new capture state when it changed.
    pub fn focus(&mut self, focused: bool) -> Option<bool> {
        self.focused = focused;
        self.swallow_release = false;
        self.set(focused && !self.given_back)
    }

    /// Escape was pressed; the game still gets the key.
    pub fn escape(&mut self) -> Option<bool> {
        if self.release_on_escape && self.captured {
            self.given_back = true;
            return self.set(false);
        }
        None
    }

    /// A mouse button changed; returns what to do with it and the new capture state if that changed.
    pub fn button(&mut self, pressed: bool) -> (Click, Option<bool>) {
        if !pressed && self.swallow_release {
            self.swallow_release = false;
            return (Click::Swallow { capture: false }, None);
        }
        if !self.captured {
            if pressed && self.focused {
                self.given_back = false;
                self.swallow_release = true;
                return (Click::Swallow { capture: true }, self.set(true));
            }
            return (Click::Swallow { capture: false }, None);
        }
        (Click::Forward, None)
    }
}

struct Shared {
    state: Mutex<WindowState>,
    sink: Mutex<Option<EventSink>>,
    motion: Mutex<Option<MotionSink>>,
}

enum Command {
    Mode(WindowMode),
    Title(String),
    Size(u32, u32),
    Close,
}

pub struct WindowOptions {
    pub title: String,
    pub size: (u32, u32),
    pub mode: WindowMode,
    /// F11 and Alt+Enter switch between windowed and borderless.
    pub hotkeys: bool,
    /// Lock and hide the cursor while the window has the focus, so the mouse can turn the camera.
    pub capture_cursor: bool,
    /// Escape gives the cursor back to the system until the next click in the window.
    pub release_on_escape: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions {
            title: "Leonida".into(),
            size: (1920, 1080),
            mode: WindowMode::Windowed,
            hotkeys: true,
            capture_cursor: true,
            release_on_escape: true,
        }
    }
}

struct App {
    options: WindowOptions,
    shared: Arc<Shared>,
    window: Option<Window>,
    ready: Option<mpsc::Sender<Result<NativeWindow, String>>>,
    modifiers: ModifiersState,
    capture: Capture,
    motion: MotionAccumulator,
}

fn native_of(window: &Window) -> Result<NativeWindow, String> {
    match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
        RawWindowHandle::Win32(h) => Ok(NativeWindow {
            hwnd: h.hwnd.get(),
            hinstance: h.hinstance.map(|i| i.get()),
        }),
        other => Err(format!("unsupported window handle {other:?}")),
    }
}

fn fullscreen_for(mode: WindowMode) -> Option<Fullscreen> {
    (mode == WindowMode::Borderless).then_some(Fullscreen::Borderless(None))
}

impl App {
    fn set_mode(&mut self, mode: WindowMode) {
        if let Some(w) = &self.window {
            w.set_fullscreen(fullscreen_for(mode));
        }
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode = mode;
    }

    fn apply_capture(&mut self, captured: Option<bool>) {
        let Some(captured) = captured else {
            return;
        };
        if let Some(w) = &self.window {
            if captured {
                let locked = w
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined));
                if locked.is_err() {
                    self.capture.focus(false);
                    return;
                }
            } else {
                let _ = w.set_cursor_grab(CursorGrabMode::None);
            }
            w.set_cursor_visible(!captured);
        }
        self.motion.clear();
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cursor_captured = captured;
    }

    fn toggle(&mut self) {
        let now = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mode;
        self.set_mode(match now {
            WindowMode::Windowed => WindowMode::Borderless,
            WindowMode::Borderless => WindowMode::Windowed,
        });
    }
}

impl ApplicationHandler<Command> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(PhysicalSize::new(self.options.size.0, self.options.size.1))
            .with_fullscreen(fullscreen_for(self.options.mode));
        let made = event_loop
            .create_window(attributes)
            .map_err(|e| e.to_string())
            .and_then(|w| native_of(&w).map(|n| (w, n)));
        match made {
            Ok((window, native)) => {
                let size = window.inner_size();
                {
                    let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.resized(size.width, size.height);
                    s.scale_factor = window.scale_factor();
                }
                window.set_ime_allowed(false);
                let focused = window.has_focus();
                self.window = Some(window);
                if self.options.capture_cursor {
                    let change = self.capture.focus(focused);
                    self.apply_capture(change);
                }
                if let Some(tx) = self.ready.take() {
                    let _ = tx.send(Ok(native));
                }
            }
            Err(e) => {
                if let Some(tx) = self.ready.take() {
                    let _ = tx.send(Err(e));
                }
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Mode(m) => self.set_mode(m),
            Command::Title(t) => {
                if let Some(w) = &self.window {
                    w.set_title(&t);
                }
            }
            Command::Size(w, h) => {
                if let Some(win) = &self.window {
                    let _ = win.request_inner_size(PhysicalSize::new(w, h));
                }
            }
            Command::Close => event_loop.exit(),
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.capture.captured()
        {
            let (dx, dy) = self.motion.add(delta.0, delta.1);
            if dx != 0 || dy != 0 {
                let motion = self.shared.motion.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(f) = motion.as_ref() {
                    f(dx, dy);
                }
            }
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let mut forward = true;
        if self.options.capture_cursor {
            match &event {
                WindowEvent::Focused(f) => {
                    let change = self.capture.focus(*f);
                    self.apply_capture(change);
                }
                WindowEvent::KeyboardInput { event: key, .. }
                    if key.state == ElementState::Pressed
                        && !key.repeat
                        && key.logical_key == Key::Named(NamedKey::Escape) =>
                {
                    let change = self.capture.escape();
                    self.apply_capture(change);
                }
                WindowEvent::MouseInput { state, .. } => {
                    let (click, change) = self.capture.button(*state == ElementState::Pressed);
                    forward = click == Click::Forward;
                    self.apply_capture(change);
                }
                WindowEvent::MouseWheel { .. } | WindowEvent::CursorMoved { .. } => {
                    forward = self.capture.captured();
                }
                _ => {}
            }
        }
        if forward {
            let sink = self.shared.sink.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(f) = sink.as_ref() {
                f(&event);
            }
        }
        match &event {
            WindowEvent::Resized(size) => {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .resized(size.width, size.height);
            }
            WindowEvent::Focused(f) => {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .focused = *f;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .scale_factor = *scale_factor;
            }
            WindowEvent::CloseRequested => {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .close_requested = true;
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. }
                if self.options.hotkeys
                    && event.state == ElementState::Pressed
                    && !event.repeat =>
            {
                let alt_enter =
                    self.modifiers.alt_key() && event.logical_key == Key::Named(NamedKey::Enter);
                if alt_enter || event.logical_key == Key::Named(NamedKey::F11) {
                    self.toggle();
                }
            }
            _ => {}
        }
    }
}

/// A window on its own thread. Dropping it closes the window.
pub struct WindowHost {
    shared: Arc<Shared>,
    proxy: EventLoopProxy<Command>,
    thread: Option<JoinHandle<()>>,
    native: NativeWindow,
}

impl WindowHost {
    pub fn open(options: WindowOptions) -> Result<WindowHost, PresentError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(WindowState::new(options.size, options.mode)),
            sink: Mutex::new(None),
            motion: Mutex::new(None),
        });
        let (ready_tx, ready_rx) = mpsc::channel();
        let (proxy_tx, proxy_rx) = mpsc::channel();
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("leonida-window".into())
            .spawn(move || {
                let mut builder = EventLoop::<Command>::with_user_event();
                #[cfg(windows)]
                {
                    use winit::platform::windows::EventLoopBuilderExtWindows;
                    builder.with_any_thread(true);
                }
                let event_loop = match builder.build() {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = proxy_tx.send(event_loop.create_proxy());
                let release_on_escape = options.release_on_escape;
                let mut app = App {
                    options,
                    shared: thread_shared,
                    window: None,
                    ready: Some(ready_tx),
                    modifiers: ModifiersState::default(),
                    capture: Capture::new(release_on_escape),
                    motion: MotionAccumulator::default(),
                };
                let _ = event_loop.run_app(&mut app);
            })
            .map_err(|e| PresentError::Window(e.to_string()))?;
        let native = ready_rx
            .recv()
            .map_err(|_| {
                PresentError::Window("the window thread ended before a window existed".into())
            })?
            .map_err(PresentError::Window)?;
        let proxy = proxy_rx
            .recv()
            .map_err(|_| PresentError::Window("no event loop proxy".into()))?;
        Ok(WindowHost {
            shared,
            proxy,
            thread: Some(thread),
            native,
        })
    }

    pub fn native(&self) -> NativeWindow {
        self.native
    }

    pub fn state(&self) -> WindowState {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_mode(&self, mode: WindowMode) {
        let _ = self.proxy.send_event(Command::Mode(mode));
    }

    pub fn set_title(&self, title: &str) {
        let _ = self.proxy.send_event(Command::Title(title.to_string()));
    }

    pub fn set_size(&self, width: u32, height: u32) {
        let _ = self.proxy.send_event(Command::Size(width, height));
    }

    /// Raw mouse movement goes to `sink` on the window thread while the cursor is captured. Wire it to
    /// `shims::gameinput::submit_input(RawInput::MouseMove { dx, dy })`.
    pub fn set_motion_sink(&self, sink: MotionSink) {
        *self.shared.motion.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    /// Every window event is passed to `sink` on the window thread before the window acts on it (input reads these).
    pub fn set_event_sink(&self, sink: EventSink) {
        *self.shared.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }
}

impl Drop for WindowHost {
    fn drop(&mut self) {
        let _ = self.proxy.send_event(Command::Close);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_sized_window_is_minimised_and_keeps_its_last_size() {
        let mut s = WindowState::new((1280, 720), WindowMode::Windowed);
        assert!(!s.minimized);
        s.resized(0, 0);
        assert!(s.minimized);
        assert_eq!(s.size, (1280, 720));
        s.resized(1280, 720);
        assert!(!s.minimized);
        assert_eq!(s.resizes, 0);
    }

    #[test]
    fn resizes_are_counted_only_when_the_size_changes() {
        let mut s = WindowState::new((1280, 720), WindowMode::Windowed);
        s.resized(1280, 720);
        s.resized(1920, 1080);
        s.resized(1920, 1080);
        s.resized(800, 600);
        assert_eq!(s.resizes, 2);
        assert_eq!(s.size, (800, 600));
    }

    #[test]
    fn motion_keeps_the_fraction_a_whole_count_cannot_carry() {
        let mut m = MotionAccumulator::default();
        assert_eq!(m.add(0.4, -0.4), (0, 0));
        assert_eq!(m.add(0.4, -0.4), (0, 0));
        assert_eq!(m.add(0.4, -0.4), (1, -1));
        let total: i32 = (0..100).map(|_| m.add(0.3, 0.0).0).sum();
        assert_eq!(total, 30);
        m.clear();
        assert_eq!(m.add(0.0, 0.0), (0, 0));
    }

    #[test]
    fn focus_captures_and_losing_it_releases() {
        let mut c = Capture::new(true);
        assert_eq!(c.focus(true), Some(true));
        assert!(c.captured());
        assert_eq!(c.focus(true), None);
        assert_eq!(c.focus(false), Some(false));
        assert!(!c.captured());
        assert_eq!(c.focus(true), Some(true));
    }

    #[test]
    fn escape_gives_the_cursor_back_and_a_click_takes_it_again_without_firing() {
        let mut c = Capture::new(true);
        c.focus(true);
        assert_eq!(c.button(true), (Click::Forward, None));
        assert_eq!(c.button(false), (Click::Forward, None));
        assert_eq!(c.escape(), Some(false));
        assert_eq!(
            c.button(true),
            (Click::Swallow { capture: true }, Some(true))
        );
        assert_eq!(c.button(false), (Click::Swallow { capture: false }, None));
        assert_eq!(c.button(true), (Click::Forward, None));
    }

    #[test]
    fn after_escape_alt_tab_and_back_leaves_the_cursor_free_until_a_click() {
        let mut c = Capture::new(true);
        c.focus(true);
        c.escape();
        c.focus(false);
        assert_eq!(c.focus(true), None);
        assert!(!c.captured());
        assert_eq!(c.button(true).1, Some(true));
    }

    #[test]
    fn clicks_while_released_never_reach_the_game_and_escape_can_be_turned_off() {
        let mut c = Capture::new(true);
        c.focus(true);
        c.escape();
        c.focus(false);
        assert_eq!(c.button(true), (Click::Swallow { capture: false }, None));
        let mut keep = Capture::new(false);
        keep.focus(true);
        assert_eq!(keep.escape(), None);
        assert!(keep.captured());
    }

    #[test]
    fn borderless_maps_to_a_fullscreen_request_and_windowed_to_none() {
        assert!(fullscreen_for(WindowMode::Windowed).is_none());
        assert!(matches!(
            fullscreen_for(WindowMode::Borderless),
            Some(Fullscreen::Borderless(None))
        ));
    }
}
