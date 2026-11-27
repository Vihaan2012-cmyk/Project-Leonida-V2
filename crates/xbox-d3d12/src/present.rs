//! Presentation: how the title's back buffers reach a window, and the frame events it waits on.
//!
//! The console title never makes a swap chain. It creates back buffers as ordinary committed resources, asks the device
//! for a frame interval and an origin event, waits on that event for a token each frame, renders, and hands the buffer
//! to the queue's `PresentX` with that token. `Presentation` is the object the device and queue slots forward to
//! (`SetFrameIntervalX`, `ScheduleFrameEventX`, `WaitFrameEventX`, `GetFrameStatisticsX`, `PresentX`, `SuspendX`,
//! `ResumeX`). The window is the layer's own: the game's Win32 calls go to the host's user32, and `PresentX` carries no
//! window handle, so frames are shown in a `winit` window (or any `HWND` passed in) through a Vulkan swap chain.

pub mod convert;
pub mod frames;
