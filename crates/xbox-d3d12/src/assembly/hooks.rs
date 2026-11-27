//! Registration points for what lives outside this crate: where the window's input goes, which content filter
//! the layer calls, and the Vulkan device an upscaler can share. They are plain functions and values, so this crate
//! depends on neither the shims nor the runtime.

use std::ops::Deref;
use std::sync::{Arc, RwLock};

use content_filter::ContentFilter;
use winit::event::{ElementState, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::PhysicalKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Keyboard and mouse input from the presentation window, in the shape the game-input layer takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key {
        virtual_key: u8,
        scan_code: u32,
        down: bool,
    },
    /// Relative movement in counts, positive down and right.
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    MouseWheel {
        dx: i32,
        dy: i32,
    },
    /// The window gained or lost focus.
    Focus(bool),
}

pub type InputSink = fn(InputEvent);

static INPUT: RwLock<Option<InputSink>> = RwLock::new(None);

/// Where the presentation window's input events go. Call it before the device exists; a window opened earlier
/// forwards to whatever is set when an event arrives.
pub fn set_input_sink(sink: InputSink) {
    *INPUT.write().unwrap_or_else(|e| e.into_inner()) = Some(sink);
}

pub fn clear_input_sink() {
    *INPUT.write().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(super) fn forward(event: InputEvent) {
    let sink = *INPUT.read().unwrap_or_else(|e| e.into_inner());
    if let Some(sink) = sink {
        sink(event);
    }
}

#[cfg(windows)]
fn virtual_key(scan_code: u32) -> u8 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK_EX, MapVirtualKeyW};
    // SAFETY: a pure lookup in the keyboard layout.
    unsafe { MapVirtualKeyW(scan_code, MAPVK_VSC_TO_VK_EX) as u8 }
}

#[cfg(not(windows))]
fn virtual_key(_scan_code: u32) -> u8 {
    0
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn scan_code(key: PhysicalKey) -> Option<u32> {
    use winit::platform::scancode::PhysicalKeyExtScancode;
    key.to_scancode()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn scan_code(_key: PhysicalKey) -> Option<u32> {
    None
}

/// Tracks the cursor so its position becomes movement.
#[derive(Default)]
pub struct Translator {
    cursor: Option<(f64, f64)>,
}

impl Translator {
    pub fn translate(&mut self, event: &WindowEvent) -> Option<InputEvent> {
        match event {
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                let scan = scan_code(event.physical_key)?;
                Some(InputEvent::Key {
                    virtual_key: virtual_key(scan),
                    scan_code: scan,
                    down: event.state == ElementState::Pressed,
                })
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    WinitButton::Left => MouseButton::Left,
                    WinitButton::Right => MouseButton::Right,
                    WinitButton::Middle => MouseButton::Middle,
                    WinitButton::Back => MouseButton::X1,
                    WinitButton::Forward => MouseButton::X2,
                    WinitButton::Other(_) => return None,
                };
                Some(InputEvent::MouseButton {
                    button,
                    down: *state == ElementState::Pressed,
                })
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32),
                    MouseScrollDelta::PixelDelta(p) => (p.x as i32, p.y as i32),
                };
                Some(InputEvent::MouseWheel { dx, dy })
            }
            WindowEvent::CursorMoved { position, .. } => {
                let now = (position.x, position.y);
                let before = self.cursor.replace(now)?;
                let (dx, dy) = ((now.0 - before.0) as i32, (now.1 - before.1) as i32);
                (dx != 0 || dy != 0).then_some(InputEvent::MouseMove { dx, dy })
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                None
            }
            WindowEvent::Focused(focus) => {
                if !focus {
                    self.cursor = None;
                }
                Some(InputEvent::Focus(*focus))
            }
            _ => None,
        }
    }
}

// content filter

static FILTER: RwLock<Option<Arc<ContentFilter>>> = RwLock::new(None);

/// The filter the layer calls on resource creation, upload, texture binding and draw. It takes the place of the
/// installed global filter (`content_filter::global`) until cleared with `None`.
pub fn set_content_filter(filter: Option<Arc<ContentFilter>>) {
    *FILTER.write().unwrap_or_else(|e| e.into_inner()) = filter;
}

/// The filter in force, when it blocks anything.
pub enum FilterHandle {
    Set(Arc<ContentFilter>),
    Global(&'static ContentFilter),
}

impl Deref for FilterHandle {
    type Target = ContentFilter;

    fn deref(&self) -> &ContentFilter {
        match self {
            FilterHandle::Set(f) => f,
            FilterHandle::Global(f) => f,
        }
    }
}

pub fn content_filter() -> Option<FilterHandle> {
    let set = FILTER.read().unwrap_or_else(|e| e.into_inner()).clone();
    match set {
        Some(f) if f.is_active() => Some(FilterHandle::Set(f)),
        Some(_) => None,
        None => content_filter::global::active().map(FilterHandle::Global),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::event::DeviceId;

    fn device() -> DeviceId {
        DeviceId::dummy()
    }

    #[test]
    fn cursor_positions_become_movement_and_the_first_one_only_sets_the_origin() {
        let mut t = Translator::default();
        let moved = |x, y| WindowEvent::CursorMoved {
            device_id: device(),
            position: PhysicalPosition::new(x, y),
        };
        assert_eq!(t.translate(&moved(10.0, 10.0)), None);
        assert_eq!(
            t.translate(&moved(15.0, 7.0)),
            Some(InputEvent::MouseMove { dx: 5, dy: -3 })
        );
        assert_eq!(t.translate(&moved(15.0, 7.0)), None);
        t.translate(&WindowEvent::Focused(false));
        assert_eq!(t.translate(&moved(100.0, 100.0)), None);
    }

    #[test]
    fn buttons_wheel_and_focus_translate() {
        let mut t = Translator::default();
        let button = |b| WindowEvent::MouseInput {
            device_id: device(),
            state: ElementState::Pressed,
            button: b,
        };
        assert_eq!(
            t.translate(&button(WinitButton::Back)),
            Some(InputEvent::MouseButton {
                button: MouseButton::X1,
                down: true
            })
        );
        assert_eq!(t.translate(&button(WinitButton::Other(9))), None);
        let wheel = WindowEvent::MouseWheel {
            device_id: device(),
            delta: MouseScrollDelta::LineDelta(0.0, -1.0),
            phase: winit::event::TouchPhase::Moved,
        };
        assert_eq!(
            t.translate(&wheel),
            Some(InputEvent::MouseWheel { dx: 0, dy: -120 })
        );
        assert_eq!(
            t.translate(&WindowEvent::Focused(true)),
            Some(InputEvent::Focus(true))
        );
    }

    #[test]
    fn an_inactive_set_filter_hides_the_global_one() {
        set_content_filter(Some(Arc::new(ContentFilter::disabled())));
        assert!(content_filter().is_none());
        set_content_filter(None);
    }
}
