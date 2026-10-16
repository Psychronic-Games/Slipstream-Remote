//! System tray icon, so Slipstream keeps running (and reachable) when its
//! window is closed. KDE/GNOME/etc. via StatusNotifierItem on Linux,
//! native tray on Windows and macOS.

use std::sync::mpsc;

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    Quit,
}

pub struct Tray {
    rx: mpsc::Receiver<TrayCmd>,
    #[cfg(target_os = "linux")]
    handle: ksni::Handle<linux::SlipTray>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    icon: tray_icon::TrayIcon,
}

impl Tray {
    pub fn poll(&self) -> Option<TrayCmd> {
        self.rx.try_recv().ok()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub struct SlipTray {
        pub tx: mpsc::Sender<TrayCmd>,
        pub ctx: egui::Context,
        pub status: String,
        pub icon: ksni::Icon,
    }

    impl SlipTray {
        fn send(&self, cmd: TrayCmd) {
            let _ = self.tx.send(cmd);
            self.ctx.request_repaint();
        }
    }

    impl ksni::Tray for SlipTray {
        fn id(&self) -> String {
            "slipstream-remote".into()
        }
        fn title(&self) -> String {
            "Slipstream Remote".into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![self.icon.clone()]
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Slipstream Remote".into(),
                description: self.status.clone(),
                ..Default::default()
            }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(TrayCmd::Show);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem { label: self.status.clone(), enabled: false, ..Default::default() }.into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Open Slipstream".into(),
                    activate: Box::new(|t: &mut Self| t.send(TrayCmd::Show)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Quit".into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|t: &mut Self| t.send(TrayCmd::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }
}

impl Tray {
    #[cfg(target_os = "linux")]
    pub fn new(rt: &tokio::runtime::Runtime, ctx: &egui::Context) -> Option<Self> {
        use ksni::TrayMethods;
        let (tx, rx) = mpsc::channel();
        let src = super::theme::icon(64);
        // ksni wants ARGB32 in network byte order.
        let data = src.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
        let tray = linux::SlipTray {
            tx,
            ctx: ctx.clone(),
            status: "Starting…".into(),
            icon: ksni::Icon { width: 64, height: 64, data },
        };
        match rt.block_on(tray.spawn()) {
            Ok(handle) => Some(Self { rx, handle }),
            Err(e) => {
                tracing::warn!("no system tray available: {e}");
                None
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub fn set_status(&self, rt: &tokio::runtime::Runtime, status: String) {
        let handle = self.handle.clone();
        rt.spawn(async move {
            handle.update(|t| t.status = status).await;
        });
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn new(_rt: &tokio::runtime::Runtime, ctx: &egui::Context) -> Option<Self> {
        use tray_icon::{
            TrayIconBuilder, TrayIconEvent,
            menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
        };
        let (tx, rx) = mpsc::channel();
        let open = MenuItem::new("Open Slipstream", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::with_items(&[&open, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let cmd = if e.id == quit_id { TrayCmd::Quit } else if e.id == open_id { TrayCmd::Show } else { return };
                let _ = tx.send(cmd);
                ctx.request_repaint();
            }));
        }
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if matches!(e, TrayIconEvent::DoubleClick { .. }) {
                    let _ = tx.send(TrayCmd::Show);
                    ctx.request_repaint();
                }
            }));
        }
        let src = super::theme::icon(64);
        let icon = tray_icon::Icon::from_rgba(src.rgba, 64, 64).ok()?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("Slipstream Remote")
            .build()
            .map_err(|e| tracing::warn!("no system tray available: {e}"))
            .ok()?;
        Some(Self { rx, icon })
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn set_status(&self, _rt: &tokio::runtime::Runtime, status: String) {
        let _ = self.icon.set_tooltip(Some(format!("Slipstream Remote — {status}")));
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    pub fn new(_rt: &tokio::runtime::Runtime, _ctx: &egui::Context) -> Option<Self> {
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    pub fn set_status(&self, _rt: &tokio::runtime::Runtime, _status: String) {}
}
