use anyhow::Result;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tokio::sync::watch;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

const ID_TOGGLE: &str = "toggle_broker";
const ID_QUIT: &str = "quit_app";

/// Create a smooth anti-aliased circular dot icon in RGBA format.
pub fn create_dot_icon(r: u8, g: u8, b: u8) -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = 11.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = if dist <= radius - 1.0 {
                255
            } else if dist <= radius + 1.0 {
                ((radius + 1.0 - dist) / 2.0 * 255.0).clamp(0.0, 255.0) as u8
            } else {
                0
            };
            rgba.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid icon RGBA buffer")
}

struct TrayApp {
    auto_approve: bool,
    running: bool,
    shutdown_tx: Option<watch::Sender<bool>>,
    icon_running: Icon,
    icon_stopped: Icon,
    tray_icon: Option<TrayIcon>,
    menu_header: MenuItem,
    menu_toggle: MenuItem,
    tokio_handle: tokio::runtime::Handle,
}

impl TrayApp {
    fn new(auto_approve: bool, tokio_handle: tokio::runtime::Handle) -> Self {
        let menu_header = MenuItem::new("🟢 SafeHell: Running", false, None);
        let menu_toggle = MenuItem::with_id(ID_TOGGLE, "⏹️  Stop Broker", true, None);
        let icon_running = create_dot_icon(34, 197, 94); // #22c55e (Green)
        let icon_stopped = create_dot_icon(239, 68, 68); // #ef4444 (Red)

        Self {
            auto_approve,
            running: false,
            shutdown_tx: None,
            icon_running,
            icon_stopped,
            tray_icon: None,
            menu_header,
            menu_toggle,
            tokio_handle,
        }
    }

    fn start_broker(&mut self) {
        if self.running {
            return;
        }
        let (tx, rx) = watch::channel(false);
        self.shutdown_tx = Some(tx);
        self.running = true;

        let auto_approve = self.auto_approve;
        self.tokio_handle.spawn(async move {
            if let Err(error) = crate::broker::serve_with_shutdown(auto_approve, rx).await {
                eprintln!("broker error: {error:#}");
            }
        });

        self.menu_header.set_text("🟢 SafeHell: Running");
        self.menu_toggle.set_text("⏹️  Stop Broker");
        if let Some(tray) = &self.tray_icon {
            let _ = tray.set_icon(Some(self.icon_running.clone()));
            let _ = tray.set_tooltip(Some("SafeHell: Running"));
        }
    }

    fn stop_broker(&mut self) {
        if !self.running {
            return;
        }
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.running = false;

        self.menu_header.set_text("🔴 SafeHell: Stopped");
        self.menu_toggle.set_text("▶️  Start Broker");
        if let Some(tray) = &self.tray_icon {
            let _ = tray.set_icon(Some(self.icon_stopped.clone()));
            let _ = tray.set_tooltip(Some("SafeHell: Stopped"));
        }
    }

    fn toggle_broker(&mut self) {
        if self.running {
            self.stop_broker();
        } else {
            self.start_broker();
        }
    }
}

impl ApplicationHandler for TrayApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray_icon.is_some() {
            return;
        }

        let menu = Menu::new();
        let menu_quit = MenuItem::with_id(ID_QUIT, "❌ Quit", true, None);
        let _ = menu.append_items(&[
            &self.menu_header,
            &PredefinedMenuItem::separator(),
            &self.menu_toggle,
            &PredefinedMenuItem::separator(),
            &menu_quit,
        ]);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("SafeHell: Running")
            .with_icon(self.icon_running.clone())
            .build()
            .expect("cannot create system tray icon");

        self.tray_icon = Some(tray);
        self.start_broker();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(50),
        ));

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == ID_TOGGLE {
                self.toggle_broker();
            } else if event.id == ID_QUIT {
                self.stop_broker();
                event_loop.exit();
            }
        }

        while let Ok(_event) = TrayIconEvent::receiver().try_recv() {}
    }
}

pub fn run(auto_approve: bool) -> Result<()> {
    let tokio_handle = tokio::runtime::Handle::current();
    let event_loop = EventLoop::new()?;
    let mut app = TrayApp::new(auto_approve, tokio_handle);
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_valid_rgba_icons() {
        let _green = create_dot_icon(34, 197, 94);
        let _red = create_dot_icon(239, 68, 68);
    }
}
