//! The window the frames are shown in: a `winit` event loop on its own thread and the state it reports.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Fullscreen, Window, WindowId};

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

struct Shared {
    state: Mutex<WindowState>,
    sink: Mutex<Option<EventSink>>,
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
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions {
            title: "Leonida".into(),
            size: (1920, 1080),
            mode: WindowMode::Windowed,
            hotkeys: true,
        }
    }
}

struct App {
    options: WindowOptions,
    shared: Arc<Shared>,
    window: Option<Window>,
    ready: Option<mpsc::Sender<Result<NativeWindow, String>>>,
    modifiers: ModifiersState,
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
                self.window = Some(window);
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

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        {
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
                let mut app = App {
                    options,
                    shared: thread_shared,
                    window: None,
                    ready: Some(ready_tx),
                    modifiers: ModifiersState::default(),
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
    fn borderless_maps_to_a_fullscreen_request_and_windowed_to_none() {
        assert!(fullscreen_for(WindowMode::Windowed).is_none());
        assert!(matches!(
            fullscreen_for(WindowMode::Borderless),
            Some(Fullscreen::Borderless(None))
        ));
    }
}
