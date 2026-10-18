//! The file transfer window: this computer on the left, the remote one on
//! the right, transfers in both directions with progress.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

use eframe::egui::{self, Align, Align2, Color32, Layout, Rect, RichText, Sense, Ui, vec2};
use slipstream_core::proto::{DirListing, Place, UploadTarget};
use slipstream_session::{RemoteFs, fsops};

use super::{theme, theme::pal};

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Preparing,
    Running,
    Done(String),
    Failed(String),
    Cancelled,
}

/// One transfer (a file, or a folder with everything in it).
pub struct Job {
    pub label: String,
    pub upload: bool,
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub state: Mutex<JobState>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Job {
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 { 0.0 } else { self.done.load(Ordering::Relaxed) as f32 / t as f32 }
    }

    pub fn is_active(&self) -> bool {
        matches!(*self.state.lock().unwrap(), JobState::Preparing | JobState::Running)
    }

    fn cancel(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
        *self.state.lock().unwrap() = JobState::Cancelled;
    }
}

enum Msg {
    RemoteListing(Result<DirListing, String>),
    RemotePlaces(Vec<Place>),
    JobFinished { upload: bool },
}

#[derive(Default)]
struct Pane {
    listing: Option<DirListing>,
    path_edit: String,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    error: Option<String>,
    loading: bool,
    places: Vec<Place>,
    rect: Option<Rect>,
    layer: Option<egui::LayerId>,
}

impl Pane {
    fn set_listing(&mut self, l: DirListing) {
        self.path_edit = l.path.clone();
        self.listing = Some(l);
        self.selected.clear();
        self.anchor = None;
        self.error = None;
        self.loading = false;
    }

    fn child(&self, name: &str) -> Option<String> {
        let l = self.listing.as_ref()?;
        Some(format!("{}{}{}", l.path.trim_end_matches(['/', '\\']), l.sep, name))
    }
}

pub struct FileManager {
    pub open: bool,
    fs: RemoteFs,
    remote_name: String,
    local: Pane,
    remote: Pane,
    pub jobs: Vec<Arc<Job>>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    started: bool,
    show_hidden: bool,
}

impl FileManager {
    pub fn new(fs: RemoteFs, remote_name: String) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            open: false,
            fs,
            remote_name,
            local: Pane::default(),
            remote: Pane::default(),
            jobs: Vec::new(),
            tx,
            rx,
            started: false,
            show_hidden: false,
        }
    }

    fn load_local(&mut self, path: Option<String>) {
        match fsops::list(path.as_deref()) {
            Ok(l) => self.local.set_listing(l),
            Err(e) => self.local.error = Some(format!("{e:#}")),
        }
    }

    fn load_remote(&mut self, path: Option<String>, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        self.remote.loading = true;
        let (fs, tx, ctx) = (self.fs.clone(), self.tx.clone(), ctx.clone());
        rt.spawn(async move {
            let r = fs.list(path).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::RemoteListing(r));
            ctx.request_repaint();
        });
    }

    fn ensure_started(&mut self, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        self.local.places = fsops::places();
        self.load_local(None);
        self.load_remote(None, rt, ctx);
        let (fs, tx, ctx) = (self.fs.clone(), self.tx.clone(), ctx.clone());
        rt.spawn(async move {
            if let Ok(p) = fs.places().await {
                let _ = tx.send(Msg::RemotePlaces(p));
                ctx.request_repaint();
            }
        });
    }

    /// Aggregate progress of active transfers, for the toolbar.
    pub fn activity(&self) -> Option<(usize, f32)> {
        let active: Vec<_> = self.jobs.iter().filter(|j| j.is_active()).collect();
        if active.is_empty() {
            return None;
        }
        let (done, total) = active.iter().fold((0u64, 0u64), |(d, t), j| {
            (d + j.done.load(Ordering::Relaxed), t + j.total.load(Ordering::Relaxed))
        });
        Some((active.len(), if total == 0 { 0.0 } else { done as f32 / total as f32 }))
    }

    /// Uploads local files/folders to `target` on the remote computer.
    pub fn upload(&mut self, items: Vec<PathBuf>, target: UploadTarget, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        self.start_upload(items, target, None, rt, ctx);
    }

    pub fn upload_to_screen(&mut self, items: Vec<PathBuf>, point: (i32, i32), rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        self.start_upload(items, UploadTarget::Default, Some(point), rt, ctx);
    }

    fn start_upload(&mut self, items: Vec<PathBuf>, target: UploadTarget, point: Option<(i32, i32)>, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        if items.is_empty() { return; }
        let label = match items.as_slice() {
            [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            many => format!("{} items", many.len()),
        };
        let job = Arc::new(Job {
            label: format!("↑ {label}"),
            upload: true,
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
            state: Mutex::new(JobState::Preparing),
            task: Mutex::new(None),
        });
        let (fs, tx, ctx2, j) = (self.fs.clone(), self.tx.clone(), ctx.clone(), job.clone());
        let task = rt.spawn(async move {
            let result: anyhow::Result<String> = async {
                let target = match point {
                    Some((x,y)) => fs.drop_target(x,y).await?,
                    None => target,
                };
                let trees = tokio::task::spawn_blocking(move || {
                    items.iter().map(|p| fsops::walk_local_tree(p)).collect::<anyhow::Result<Vec<_>>>()
                }).await??;
                if let UploadTarget::Folder(base) = &target {
                    for tree in &trees {
                        for dir in &tree.directories {
                            fs.make_dir(format!("{}/{dir}", base.trim_end_matches(['/', '\\']))).await?;
                        }
                    }
                }
                let folders = trees.iter().map(|tree| tree.directories.len()).sum::<usize>();
                let files: Vec<_> = trees.into_iter().flat_map(|tree| tree.files).collect();
                j.total.store(files.iter().map(|f| f.2).sum(), Ordering::Relaxed);
                *j.state.lock().unwrap() = JobState::Running;
                let mut last = String::new();
                for (path, rel, _) in &files {
                    last = fs
                        .upload(path, target.clone(), rel.clone(), |n| {
                            j.done.fetch_add(n, Ordering::Relaxed);
                            ctx2.request_repaint_after(std::time::Duration::from_millis(100));
                        })
                        .await?;
                }
                Ok(match files.len() {
                    0 => format!("{folders} folders sent"),
                    1 => format!("saved to {last}"),
                    n => format!("{n} files sent"),
                })
            }
            .await;
            *j.state.lock().unwrap() = match result {
                Ok(m) => JobState::Done(m),
                Err(e) => JobState::Failed(format!("{e:#}")),
            };
            let _ = tx.send(Msg::JobFinished { upload: true });
            ctx2.request_repaint();
        });
        *job.task.lock().unwrap() = Some(task);
        self.jobs.push(job);
    }

    /// Downloads the selected remote items into the current local folder.
    fn download_selected(&mut self, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        let (Some(rl), Some(ll)) = (&self.remote.listing, &self.local.listing) else { return };
        let picks: Vec<(String, bool, String, u64)> = self
            .remote
            .selected
            .iter()
            .filter_map(|&i| rl.entries.get(i))
            .filter_map(|e| Some((self.remote.child(&e.name)?, e.is_dir, e.name.clone(), e.size)))
            .collect();
        if picks.is_empty() {
            return;
        }
        let dest_dir = PathBuf::from(&ll.path);
        let label = if picks.len() == 1 { picks[0].2.clone() } else { format!("{} items", picks.len()) };
        let job = Arc::new(Job {
            label: format!("↓ {label}"),
            upload: false,
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
            state: Mutex::new(JobState::Preparing),
            task: Mutex::new(None),
        });
        let (fs, tx, ctx2, j) = (self.fs.clone(), self.tx.clone(), ctx.clone(), job.clone());
        let task = rt.spawn(async move {
            let result: anyhow::Result<String> = async {
                let mut files = Vec::new();
                for (path, is_dir, name, size) in picks {
                    files.extend(fs.walk(path, is_dir, name, size).await?);
                }
                j.total.store(files.iter().map(|f| f.2).sum(), Ordering::Relaxed);
                *j.state.lock().unwrap() = JobState::Running;
                let mut last = PathBuf::new();
                for (remote, rel, _) in &files {
                    let dest = dest_dir.join(fsops::safe_relative(rel)?);
                    last = fs
                        .download(remote.clone(), &dest, |n| {
                            j.done.fetch_add(n, Ordering::Relaxed);
                            ctx2.request_repaint_after(std::time::Duration::from_millis(100));
                        })
                        .await?;
                }
                Ok(match files.len() {
                    1 => format!("saved to {}", last.display()),
                    n => format!("{n} files received"),
                })
            }
            .await;
            *j.state.lock().unwrap() = match result {
                Ok(m) => JobState::Done(m),
                Err(e) => JobState::Failed(format!("{e:#}")),
            };
            let _ = tx.send(Msg::JobFinished { upload: false });
            ctx2.request_repaint();
        });
        *job.task.lock().unwrap() = Some(task);
        self.jobs.push(job);
    }

    fn handle_messages(&mut self, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::RemoteListing(Ok(l)) => self.remote.set_listing(l),
                Msg::RemoteListing(Err(e)) => {
                    self.remote.error = Some(e);
                    self.remote.loading = false;
                }
                Msg::RemotePlaces(p) => self.remote.places = p,
                Msg::JobFinished { upload } => {
                    // Show the new files where they landed.
                    if upload {
                        let p = self.remote.listing.as_ref().map(|l| l.path.clone());
                        if p.is_some() {
                            self.load_remote(p, rt, ctx);
                        }
                    } else {
                        let p = self.local.listing.as_ref().map(|l| l.path.clone());
                        self.load_local(p);
                    }
                }
            }
        }
    }

    /// Where dropped files should go, if dropped on the remote pane.
    pub fn remote_drop_target(&self, pos: egui::Pos2, ctx: &egui::Context) -> Option<UploadTarget> {
        (self.open && self.remote.rect.is_some_and(|r| r.contains(pos))
            && ctx.layer_id_at(pos).is_some_and(|layer| Some(layer) == self.remote.layer))
            .then(|| self.remote.listing.as_ref().map(|l| UploadTarget::Folder(l.path.clone())))
            .flatten()
    }

    pub fn ui(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Runtime) {
        self.handle_messages(rt, ctx);
        if !self.open {
            return;
        }
        self.ensure_started(rt, ctx);
        let mut open = self.open;
        let screen = ctx.content_rect();
        let size = vec2((screen.width() - 60.0).min(1000.0), (screen.height() - 90.0).min(620.0));
        theme::window("file-transfer", pal::ORANGE)
            .id(egui::Id::unique("file-transfer"))
            .default_pos(egui::pos2(screen.center().x - size.x / 2.0, screen.top() + 56.0))
            .default_size(size)
            .max_size(vec2(screen.width() - 30.0, screen.height() - 66.0))
            .min_size([640.0, 380.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let chrome = theme::window_header(ui, "File transfer", pal::ORANGE);
                if chrome.close {
                    open = false;
                }
                self.contents(ui, rt, &chrome)
            });
        self.open = open;
    }

    fn contents(&mut self, ui: &mut Ui, rt: &tokio::runtime::Runtime, chrome: &theme::WindowChrome) {
        // Panels take the window's space as it is, whatever their contents
        // measure: sizing these from estimates made the window grow a little
        // every frame, back to the bottom of the screen after any resize.
        egui::Panel::bottom("file-footer").frame(egui::Frame::NONE).show(ui, |ui| chrome.footer(ui));
        if !self.jobs.is_empty() {
            egui::Panel::bottom("file-jobs").frame(egui::Frame::NONE).show(ui, |ui| self.jobs_ui(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| self.panes(ui, rt));
    }

    fn panes(&mut self, ui: &mut Ui, rt: &tokio::runtime::Runtime) {
        let ctx = ui.ctx().clone();
        let avail = ui.available_size();
        // A little under the space given: text heights round up by a
        // fraction of a pixel, and any overshoot grows the window by that
        // much every frame.
        let pane_h = (avail.y - 2.0).max(160.0);
        let mid_w = 120.0;
        let pane_w = ((avail.x - mid_w - 16.0) / 2.0).max(220.0);

        let (mut go_local, mut go_remote) = (None::<Option<String>>, None::<Option<String>>);
        let (mut send, mut receive) = (false, false);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(pane_w, pane_h), Layout::top_down(Align::Min), |ui| {
                ui.set_width(pane_w);
                ui.set_height(pane_h);
                go_local = pane_ui(ui, &mut self.local, "This computer", pal::BLUE, "local", self.show_hidden);
            });
            ui.allocate_ui_with_layout(vec2(mid_w, pane_h), Layout::top_down(Align::Center), |ui| {
                ui.set_width(mid_w);
                ui.add_space(pane_h / 2.0 - 60.0);
                ui.add_enabled_ui(!self.local.selected.is_empty() && self.remote.listing.is_some(), |ui| {
                    send = theme::pill_sized(ui, "Send ▶", pal::ORANGE, vec2(110.0, 36.0))
                        .on_hover_text("Copy the selected files to the remote folder")
                        .clicked();
                });
                ui.add_space(12.0);
                ui.add_enabled_ui(!self.remote.selected.is_empty() && self.local.listing.is_some(), |ui| {
                    receive = theme::pill_sized(ui, "◀ Receive", pal::BLUE, vec2(110.0, 36.0))
                        .on_hover_text("Copy the selected remote files here")
                        .clicked();
                });
                ui.add_space(24.0);
                if ui.checkbox(&mut self.show_hidden, "Show hidden").changed() {
                    self.local.selected.clear();
                    self.remote.selected.clear();
                }
            });
            ui.allocate_ui_with_layout(vec2(pane_w, pane_h), Layout::top_down(Align::Min), |ui| {
                ui.set_width(pane_w);
                ui.set_height(pane_h);
                let title = format!("Remote · {}", self.remote_name);
                go_remote = pane_ui(ui, &mut self.remote, &title, pal::ORANGE, "remote", self.show_hidden);
            });
        });
        if let Some(p) = go_local {
            self.load_local(p);
        }
        if let Some(p) = go_remote {
            self.load_remote(p, rt, &ctx);
        }
        if send && let (Some(l), Some(r)) = (&self.local.listing, &self.remote.listing) {
            let items: Vec<PathBuf> = self.local.selected.iter().filter_map(|&i| l.entries.get(i)).map(|e| PathBuf::from(&l.path).join(&e.name)).collect();
            let target = UploadTarget::Folder(r.path.clone());
            self.upload(items, target, rt, &ctx);
        }
        if receive {
            self.download_selected(rt, &ctx);
        }
    }

    /// The transfers list under the panes.
    fn jobs_ui(&mut self, ui: &mut Ui) {
        {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("TRANSFERS").font(theme::display(16.0)).color(pal::LAVENDER));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Clear finished").clicked() {
                        self.jobs.retain(|j| j.is_active());
                    }
                });
            });
            // Four rows, then it scrolls.
            egui::ScrollArea::vertical().id_salt("jobs").max_height(4.0 * 30.0).show(ui, |ui| {
                for job in self.jobs.iter().rev() {
                    ui.horizontal(|ui| {
                        let state = job.state.lock().unwrap().clone();
                        ui.add_sized([220.0, 22.0], egui::Label::new(RichText::new(&job.label).font(theme::body(15.0))).truncate());
                        match &state {
                            JobState::Preparing => {
                                ui.label(RichText::new("preparing…").color(pal::DIM));
                            }
                            JobState::Running => {
                                let total = job.total.load(Ordering::Relaxed);
                                theme::progress(
                                    ui,
                                    job.fraction(),
                                    ui.available_width() - 200.0,
                                    if job.upload { pal::ORANGE } else { pal::BLUE },
                                    &format!("{} / {}", human_size(job.done.load(Ordering::Relaxed)), human_size(total)),
                                );
                            }
                            JobState::Done(m) => {
                                ui.label(RichText::new(format!("✔ {m}")).color(pal::GREEN));
                            }
                            JobState::Failed(e) => {
                                ui.label(RichText::new(format!("✘ {e}")).color(pal::RED));
                            }
                            JobState::Cancelled => {
                                ui.label(RichText::new("cancelled").color(pal::DIM));
                            }
                        }
                        if job.is_active() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Cancel").clicked() {
                                    job.cancel();
                                }
                            });
                        }
                    });
                }
            });
        }
    }
}

/// Draws one pane. Returns `Some(path)` to navigate (`Some(None)` = home).
fn pane_ui(ui: &mut Ui, pane: &mut Pane, title: &str, color: Color32, id: &str, show_hidden: bool) -> Option<Option<String>> {
    let mut nav: Option<Option<String>> = None;
    theme::header(ui, title, color);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(("places", id))
            .selected_text("Places")
            .width(110.0)
            .show_ui(ui, |ui| {
                for p in &pane.places {
                    if ui.selectable_label(false, &p.label).on_hover_text(&p.path).clicked() {
                        nav = Some(Some(p.path.clone()));
                    }
                }
            });
        let parent = pane.listing.as_ref().and_then(|l| l.parent.clone());
        if ui.add_enabled(parent.is_some(), egui::Button::new("▲ Up")).clicked() {
            nav = Some(parent);
        }
        if ui.button("⟳").on_hover_text("Refresh").clicked() {
            nav = Some(pane.listing.as_ref().map(|l| l.path.clone()));
        }
        let r = ui.add(egui::TextEdit::singleline(&mut pane.path_edit).align(egui::Align2::LEFT_CENTER).desired_width(ui.available_width()).font(theme::mono(14.0)));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            nav = Some(Some(pane.path_edit.clone()));
        }
    });
    if let Some(e) = &pane.error {
        ui.label(RichText::new(e).color(pal::RED));
    }
    let frame = egui::Frame::new().fill(Color32::from_rgb(4, 6, 12)).corner_radius(6.0).inner_margin(4.0);
    // Room for the "N items" line below, measured: a guess that came out
    // short made the window grow by the difference every frame.
    let footer = ui.text_style_height(&egui::TextStyle::Small) + 2.0 * ui.spacing().item_spacing.y;
    let list_h = (ui.available_height() - footer).max(80.0);
    let out = frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), list_h));
        ui.set_max_height(list_h);
        if pane.loading && pane.listing.is_none() {
            ui.label(RichText::new("Loading…").color(pal::DIM));
            return;
        }
        let Some(listing) = &pane.listing else { return };
        let row_h = 24.0;
        let modifiers = ui.input(|i| i.modifiers);
        let rows: Vec<usize> =
            (0..listing.entries.len()).filter(|&i| show_hidden || !listing.entries[i].name.starts_with('.')).collect();
        egui::ScrollArea::vertical().id_salt(("rows", id)).auto_shrink([false, false]).show_rows(
            ui,
            row_h,
            rows.len(),
            |ui, range| {
                for row in range {
                    let i = rows[row];
                    let e = &listing.entries[i];
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    let selected = pane.selected.contains(&i);
                    let p = ui.painter();
                    if selected {
                        p.rect_filled(rect, 4.0, color.gamma_multiply(0.35));
                    } else if resp.hovered() {
                        p.rect_filled(rect, 4.0, Color32::from_rgb(18, 22, 38));
                    }
                    let icon = if e.is_dir { "📁" } else { "📄" };
                    p.text(rect.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, icon, theme::body(15.0), pal::TAN);
                    let name_w = rect.width() - 190.0;
                    let galley = p.layout(e.name.clone(), theme::body(15.0), if e.is_dir { pal::TAN } else { pal::TEXT }, name_w.max(60.0));
                    let name_rect = Rect::from_min_size(rect.left_top() + vec2(30.0, (row_h - 18.0) / 2.0), vec2(name_w.max(60.0), 18.0));
                    p.with_clip_rect(name_rect).galley(name_rect.min, galley, pal::TEXT);
                    if !e.is_dir {
                        p.text(rect.right_center() - vec2(110.0, 0.0), Align2::RIGHT_CENTER, human_size(e.size), theme::mono(13.0), pal::BLUE);
                    }
                    p.text(rect.right_center() - vec2(6.0, 0.0), Align2::RIGHT_CENTER, short_date(e.modified), theme::mono(12.0), pal::DIM);
                    if resp.double_clicked() && e.is_dir {
                        nav = Some(Some(format!("{}{}{}", listing.path.trim_end_matches(['/', '\\']), listing.sep, e.name)));
                    } else if resp.clicked() {
                        if modifiers.shift
                            && let Some(a) = pane.anchor.and_then(|a| rows.iter().position(|&r| r == a))
                        {
                            pane.selected = rows[a.min(row)..=a.max(row)].iter().copied().collect();
                        } else if modifiers.command || modifiers.ctrl {
                            if !pane.selected.remove(&i) {
                                pane.selected.insert(i);
                            }
                            pane.anchor = Some(i);
                        } else {
                            pane.selected = [i].into();
                            pane.anchor = Some(i);
                        }
                    }
                }
            },
        );
    });
    pane.rect = Some(out.response.rect);
    pane.layer = Some(ui.layer_id());
    if let Some(l) = &pane.listing {
        let n = pane.selected.len();
        let bytes: u64 = pane.selected.iter().filter_map(|&i| l.entries.get(i)).map(|e| e.size).sum();
        ui.label(
            RichText::new(if n == 0 { format!("{} items", l.entries.len()) } else { format!("{n} selected · {}", human_size(bytes)) })
                .small()
                .color(pal::DIM),
        );
    }
    nav
}

pub fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

fn short_date(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(short_date(0), "");
        assert_eq!(short_date(1_792_183_720), "2026-10-16 20:48");
    }
}
