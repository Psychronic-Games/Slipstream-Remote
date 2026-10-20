//! Explicit, reviewable local reports. No credentials, chat or raw logs enter
//! the report; a remote screenshot is included only after selecting its checkbox.
use super::theme::{self, pal};
use eframe::egui::{self, RichText};
use slipstream_session::{RemoteFs, ViewerStats, YuvFrame};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

type Reply<T> = tokio::sync::oneshot::Receiver<Result<T, String>>;
#[derive(Default)]
pub struct Report {
    pub open: bool,
    notes: String,
    screenshot: bool,
    directory: String,
    inventory: Option<String>,
    inventory_at: u64,
    fetch: Option<Reply<String>>,
    fetch_job: Option<tokio::task::JoinHandle<()>>,
    save: Option<Reply<PathBuf>>,
    save_job: Option<tokio::task::JoinHandle<()>>,
    notice: Option<Result<String, String>>,
    status: String,
    status_rx: Option<Reply<()>>,
    status_job: Option<tokio::task::JoinHandle<()>>,
}

pub struct Source<'a> {
    pub id: u64,
    pub host: &'a str,
    pub connected: bool,
    pub can_control: bool,
    pub fs: RemoteFs,
    pub stats: &'a ViewerStats,
    pub network: String,
    pub frame: Option<Arc<YuvFrame>>,
}

impl Drop for Report {
    fn drop(&mut self) {
        if let Some(job) = self.fetch_job.take() {
            job.abort();
        }
        if let Some(job) = self.status_job.take() {
            job.abort();
        }
        // Blocking disk writes finish their unique report directory even if the
        // viewer closes; they never touch an existing report or remote files.
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sections(sections: &[slipstream_core::proto::InfoSection]) -> String {
    sections
        .iter()
        .map(|s| {
            format!(
                "{}\n{}\n",
                s.title,
                s.rows
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}\n"))
                    .collect::<String>()
            )
        })
        .collect()
}

impl Report {
    pub fn show(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime, source: Source<'_>) {
        if !self.open {
            return;
        }
        if self.directory.is_empty() {
            self.directory = directories::UserDirs::new()
                .map(|d| d.download_dir().unwrap_or(d.home_dir()).to_path_buf())
                .unwrap_or_else(std::env::temp_dir)
                .display()
                .to_string();
        }
        if let Some(rx) = &mut self.fetch {
            match rx.try_recv() {
                Ok(result) => {
                    self.fetch = None;
                    self.fetch_job = None;
                    self.inventory_at = now();
                    match result {
                        Ok(text) => self.inventory = Some(text),
                        Err(e) => self.notice = Some(Err(e)),
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.fetch = None;
                    self.fetch_job = None;
                    self.notice = Some(Err("Inventory request was interrupted".into()));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &mut self.save {
            match rx.try_recv() {
                Ok(result) => {
                    self.save = None;
                    self.save_job = None;
                    self.notice =
                        Some(result.map(|p| format!("Saved report folder: {}", p.display())));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.save = None;
                    self.notice = Some(Err("Report save did not finish".into()));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &mut self.status_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.status_rx = None;
                    self.status_job = None;
                    self.notice=Some(result.map(|()|"Status sent to the host app; it is shown in the signed-in desktop while that app is running".into()));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.status_rx = None;
                    self.status_job = None;
                    self.notice = Some(Err("Status request was interrupted".into()));
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        let mut open = true;
        let mut collect = false;
        let mut save = false;
        let mut status_send = None;
        theme::window("support-report",pal::BLUE).id(egui::Id::unique(("support-report",source.id))).default_width(680.0).show(ctx,|ui| {
            let chrome=theme::window_header(ui,"Support report / status",pal::BLUE);open = !chrome.close;
            egui::ScrollArea::vertical().id_salt("support-report-body")
                .max_height((ctx.content_rect().height() - 130.0).max(180.0))
                .show(ui, |ui| {
            ui.collapsing("Message shown to the person being helped",|ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("support-status-presets").selected_text("Choose a message").show_ui(ui,|ui| {
                        for text in ["Checking the printer", "Checking network connectivity", "Reviewing system information", "Please keep working; I will let you know if I need control"] {
                            if ui.button(text).clicked(){self.status=text.into();ui.close();}
                        }
                    });
                });
                ui.add(egui::TextEdit::singleline(&mut self.status).desired_width(f32::INFINITY).char_limit(160).hint_text("What are you doing?"));
                ui.horizontal_wrapped(|ui| {
                    let enabled=source.connected && source.can_control && self.status_rx.is_none();
                    if ui.add_enabled(enabled,egui::Button::new("Show status")).clicked(){status_send=Some(self.status.clone());}
                    if ui.add_enabled(enabled,egui::Button::new("Clear status")).clicked(){status_send=Some(String::new());}
                    if self.status_rx.is_some(){ui.spinner();}
                });
                ui.label(RichText::new("The host can dismiss the message or stop support. This message does not approve a change or run a command.").small().color(pal::DIM));
            });
            ui.label(RichText::new(format!("{} · {}",source.host,if source.connected {"Connected"}else{"Link interrupted; retained data"})).color(pal::LAVENDER));
            ui.label("Collect inventory, review the report, then copy it or save a new local folder. Chat, credentials and raw logs are excluded.");
            ui.horizontal_wrapped(|ui| {
                collect=ui.add_enabled(source.connected && self.fetch.is_none(),egui::Button::new("Collect / refresh inventory")).clicked();
                if self.fetch.is_some(){ui.spinner();ui.label("Collecting inventory and public IP…");}
                if self.inventory.is_some(){ui.label(format!("Inventory: {}",slipstream_core::clock::format_utc(self.inventory_at)));}
            });
            ui.label("Technician notes");ui.add(egui::TextEdit::multiline(&mut self.notes).desired_rows(3).desired_width(f32::INFINITY).char_limit(8000));
            ui.add_enabled_ui(source.frame.is_some(),|ui| {ui.checkbox(&mut self.screenshot,"Include the latest received remote-screen image (screen.png)");});
            if self.screenshot {ui.label(RichText::new("The screenshot may contain sensitive on-screen information. Check the remote screen before saving; it may be the last frame from an interrupted link.").small().color(pal::GOLD));}
            let text=self.text(&source);
            egui::CollapsingHeader::new("Review report text").default_open(true).show(ui,|ui| {
                egui::ScrollArea::vertical().id_salt("report-preview").max_height(270.0).show(ui,|ui| {let mut view=text.as_str();ui.add(egui::TextEdit::multiline(&mut view).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));});
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Copy report text").clicked(){ctx.copy_text(text.clone());}
                ui.label("Save into existing local folder");
            });
            ui.add(egui::TextEdit::singleline(&mut self.directory).desired_width(f32::INFINITY));
            save=ui.add_enabled(self.save.is_none() && self.fetch.is_none() && self.inventory.is_some(),egui::Button::new("Save report folder")).clicked();
            if self.save.is_some(){ui.spinner();}
            if let Some(result)=&self.notice {match result {Ok(s)=>{ui.label(RichText::new(s).color(pal::GREEN));},Err(e)=>{ui.label(RichText::new(e).color(pal::RED));}}}
            });
            chrome.footer(ui);
        });
        self.open = open;
        if !open {theme::repaint_console(ctx);ctx.request_discard("Support report closed");}
        if let Some(text) = status_send {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let fs = source.fs.clone();
            let wake = ctx.clone();
            self.status_rx = Some(rx);
            self.notice = None;
            self.status_job = Some(rt.spawn(async move {
                let result = fs.support_status(text).await.map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
                wake.request_repaint();
            }));
        }
        if collect {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let fs = source.fs.clone();
            let wake = ctx.clone();
            self.fetch = Some(rx);
            self.notice = None;
            self.fetch_job = Some(rt.spawn(async move {
                let (inventory, public) = tokio::join!(fs.support_inventory(), fs.public_ip(false));
                let result = inventory
                    .map(|s| {
                        format!(
                            "Public IP: {}\n\n{}",
                            public
                                .map(|ip| ip.to_string())
                                .unwrap_or_else(|_| "Unavailable".into()),
                            sections(&s)
                        )
                    })
                    .map_err(|e| format!("Inventory unavailable: {e:#}"));
                let _ = tx.send(result);
                wake.request_repaint();
            }));
        }
        if save {
            if self.screenshot && source.frame.is_none() {
                self.notice = Some(Err(
                    "No remote frame is available; clear the screenshot option to save text only"
                        .into(),
                ));
                return;
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let directory = PathBuf::from(self.directory.trim());
            let text = self.text(&source);
            let frame = if self.screenshot {
                source.frame.clone()
            } else {
                None
            };
            let wake = ctx.clone();
            self.save = Some(rx);
            self.notice = None;
            self.save_job = Some(rt.spawn_blocking(move || {
                let result = save_report(&directory, &text, frame)
                    .map_err(|e| format!("Could not save report: {e:#}"));
                let _ = tx.send(result);
                wake.request_repaint();
            }));
        }
        if self.fetch.is_some() || self.save.is_some() || self.status_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn text(&self, source: &Source<'_>) -> String {
        let s = source.stats;
        format!(
            "SLIPSTREAM SUPPORT REPORT\nViewer: {}\nWorkstation: {}\nGenerated: {}\nInventory sampled: {}\n\nTechnician notes\n{}\n\nConnection at report generation\nState: {}\nPath: {}\nPicture: {} x {}\nFPS: {:.1}; RTT: {:.1} ms; Video: {:.0} kbps; Decode: {:.1} ms\n\n{}\n\nNetwork results (retained scan/filter and diagnostics; not a fresh survey)\n{}\n\nScreenshot requested: {}\n",
            slipstream_core::version_string(),
            source.host,
            slipstream_core::clock::format_utc(now()),
            slipstream_core::clock::format_utc(self.inventory_at),
            self.notes,
            if source.connected {
                "Connected"
            } else {
                "Interrupted"
            },
            if s.relayed {
                "Relayed"
            } else {
                "Direct (last known)"
            },
            s.width,
            s.height,
            s.fps,
            s.rtt_ms,
            s.kbps,
            s.decode_ms,
            self.inventory
                .as_deref()
                .unwrap_or("Inventory not collected"),
            source.network,
            self.screenshot
        )
    }
}

fn save_report(parent: &Path, text: &str, frame: Option<Arc<YuvFrame>>) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(parent.is_dir(), "Choose an existing local folder");
    let folder = tempfile::Builder::new()
        .prefix(&format!("Slipstream-report-{}-", now()))
        .tempdir_in(parent)?;
    let write_private = |path: &Path, data: &[u8]| -> anyhow::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new();
        file.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file.mode(0o600);
        }
        file.open(path)?.write_all(data)?;
        Ok(())
    };
    write_private(&folder.path().join("report.txt"), text.as_bytes())?;
    if let Some(frame) = frame {
        let mut data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut data, frame.width, frame.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&frame.to_rgba())?;
        }
        write_private(&folder.path().join("screen.png"), &data)?;
    }
    Ok(folder.keep())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_saves_unique_private_folders_without_overwriting() {
        let base = tempfile::tempdir().unwrap();
        let first = save_report(base.path(), "first", None).unwrap();
        let second = save_report(base.path(), "second", None).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            std::fs::read_to_string(first.join("report.txt")).unwrap(),
            "first"
        );
        assert!(!first.join("screen.png").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(first.join("report.txt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(save_report(&base.path().join("missing"), "unused", None).is_err());
    }
}
