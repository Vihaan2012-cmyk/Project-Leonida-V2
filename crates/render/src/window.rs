use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

use crate::device::Queue;
use crate::error::{RenderError, Result};
use crate::swapchain::{Swapchain, SwapchainDesc};

#[derive(Debug, Clone)]
pub struct WindowDesc {
    pub title: String,
    /// Inner size in physical pixels.
    pub size: [u32; 2],
    pub resizable: bool,
}

impl Default for WindowDesc {
    fn default() -> Self {
        WindowDesc {
            title: "Leonida".into(),
            size: [1280, 720],
            resizable: true,
        }
    }
}

impl WindowDesc {
    pub fn attributes(&self) -> WindowAttributes {
        Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(PhysicalSize::new(self.size[0], self.size[1]))
            .with_resizable(self.resizable)
    }
}

/// A winit window and the swapchain that presents to it.
pub struct RenderWindow {
    pub window: Arc<Window>,
    pub swapchain: Swapchain,
}

impl RenderWindow {
    /// Creates the window on `event_loop` (call from `ApplicationHandler::resumed`) and its swapchain. The
    /// swapchain's extent is taken from the window's inner size.
    pub fn new(
        event_loop: &ActiveEventLoop,
        queue: &Queue,
        window: &WindowDesc,
        swapchain: &SwapchainDesc,
    ) -> Result<RenderWindow> {
        let window = Arc::new(
            event_loop
                .create_window(window.attributes())
                .map_err(|e| RenderError::Window(e.to_string()))?,
        );
        let size = window.inner_size();
        let desc = SwapchainDesc {
            extent: [size.width, size.height],
            ..*swapchain
        };
        let swapchain = Swapchain::new(queue, window.clone(), &desc)?;
        Ok(RenderWindow { window, swapchain })
    }

    /// Feeds a window event to the swapchain: resizes and scale changes make it recreate at the next acquire.
    pub fn handle_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::Resized(size) => self.swapchain.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = self.window.inner_size();
                self.swapchain.resize(size.width, size.height);
            }
            _ => {}
        }
    }
}
