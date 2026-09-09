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

/// Calculate shortest distance from point `(px, py)` to line segment `(x1, y1) -> (x2, y2)`.
fn dist_to_segment(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return ((px - x1) * (px - x1) + (py - y1) * (py - y1)).sqrt();
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0);
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    ((px - proj_x) * (px - proj_x) + (py - proj_y) * (py - proj_y)).sqrt()
}

/// Create SafeHell brand padlock icon with terminal prompt `>_` cut into it.
pub fn create_brand_icon(r: u8, g: u8, b: u8) -> Icon {
    const SIZE: usize = 32;
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);

    for y in 0..SIZE {
        for x in 0..SIZE {
            // 2x2 subpixel supersampling for crisp anti-aliasing
            let mut coverage = 0.0f32;
            for sy in [0.25f32, 0.75f32] {
                for sx in [0.25f32, 0.75f32] {
                    let px = x as f32 + sx;
                    let py = y as f32 + sy;

                    // 1. Shackle (arch and legs)
                    let d_arch = if py <= 11.4 && (11.0..=21.0).contains(&px) {
                        let dx = px - 16.0;
                        let dy = py - 11.4;
                        ((dx * dx + dy * dy).sqrt() - 5.0).abs()
                    } else {
                        f32::MAX
                    };
                    let d_leg_l = dist_to_segment(px, py, 11.0, 11.4, 11.0, 14.5);
                    let d_leg_r = dist_to_segment(px, py, 21.0, 11.4, 21.0, 14.5);
                    let in_shackle = d_arch.min(d_leg_l).min(d_leg_r) <= 1.6;

                    // 2. Lock Body (5.5 <= x <= 26.5, 14.0 <= y <= 27.0 with rounded corners rx=2.0)
                    let in_body_box = (5.5..=26.5).contains(&px) && (14.0..=27.0).contains(&py);
                    let in_body = if in_body_box {
                        let cx = px.clamp(7.5, 24.5);
                        let cy = py.clamp(16.0, 25.0);
                        let d_corner = ((px - cx) * (px - cx) + (py - cy) * (py - cy)).sqrt();
                        d_corner <= 2.0
                    } else {
                        false
                    };

                    // 3. Prompt cutout `>_`
                    let d_p1 = dist_to_segment(px, py, 9.6, 17.6, 13.4, 20.5);
                    let d_p2 = dist_to_segment(px, py, 13.4, 20.5, 9.6, 23.4);
                    let d_p3 = dist_to_segment(px, py, 15.8, 23.6, 22.2, 23.6);
                    let in_prompt = d_p1.min(d_p2).min(d_p3) <= 1.25;

                    if (in_shackle || in_body) && !in_prompt {
                        coverage += 0.25;
                    }
                }
            }

            let alpha = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[r, g, b, alpha]);
        }
    }

    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("valid icon RGBA buffer")
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
        let icon_running = create_brand_icon(74, 222, 128); // #4ade80 (Brand Green)
        let icon_stopped = create_brand_icon(239, 68, 68); // #ef4444 (Stop Red)

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
    fn creates_valid_brand_padlock_icons() {
        let _green = create_brand_icon(74, 222, 128);
        let _red = create_brand_icon(239, 68, 68);
    }
}
