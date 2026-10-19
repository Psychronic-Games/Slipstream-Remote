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
use slipstream_core::proto::{DirEntry, DirListing, Place, UploadTarget};
use slipstream_session::{RemoteFs, fsops};

use super::{file_previews::{Loader, PreviewCache}, theme, theme::pal};

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
    destination: Mutex<Option<String>>,
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

    fn cancel(&self) {self.stop(JobState::Cancelled);}

    fn stop(&self,outcome:JobState) {
        let mut state=self.state.lock().unwrap();
        if matches!(*state,JobState::Preparing|JobState::Running) {
            if let Some(t)=self.task.lock().unwrap().take() {t.abort();}
            *state=outcome;
        }
    }

    fn begin(&self)->bool {
        let mut state=self.state.lock().unwrap();
        if *state==JobState::Preparing {*state=JobState::Running;true}else{false}
    }

    fn finish(&self, result:JobState) {
        let mut state=self.state.lock().unwrap();
        // Cancellation/disconnection can race the final remote receipt.
        if matches!(*state,JobState::Preparing|JobState::Running) {*state=result;}
    }
}

enum Msg {
    RemoteListing(Result<DirListing, String>),
    RemotePlaces(Vec<Place>),
    JobFinished { upload: bool },
}

#[derive(Clone, Copy, Default, PartialEq)]
enum SortField { #[default] Name, Modified }

#[derive(Clone, Copy, Default, PartialEq)]
struct Sort { field: SortField, descending: bool }

impl Sort {
    fn choose(&mut self, field: SortField) {
        if self.field == field { self.descending = !self.descending; }
        else { self.field = field; self.descending = field == SortField::Modified; }
    }
}

/// Display order only: selections, transfers and anchors keep listing indices.
fn sorted_rows(entries: &[DirEntry], show_hidden: bool, sort: Sort) -> Vec<usize> {
    let mut rows: Vec<_> = entries.iter().enumerate()
        .filter(|(_,e)|show_hidden || !e.name.starts_with('.'))
        .map(|(i,e)|(i,e.name.to_lowercase())).collect();
    rows.sort_by(|(a,an),(b,bn)| {
        let (a,b)=(&entries[*a],&entries[*b]);
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            if sort.field == SortField::Modified {
                // Unknown timestamps stay last in either direction.
                (a.modified <= 0).cmp(&(b.modified <= 0)).then_with(|| {
                    if sort.descending {b.modified.cmp(&a.modified)} else {a.modified.cmp(&b.modified)}
                })
            } else {std::cmp::Ordering::Equal}
        }).then_with(|| {
            let order=an.cmp(bn).then_with(||a.name.cmp(&b.name));
            if sort.field == SortField::Name && sort.descending {order.reverse()} else {order}
        })
    });
    rows.into_iter().map(|(i,_)|i).collect()
}

#[derive(Default)]
struct Pane {
    sort: Sort,
    rows: Vec<usize>,
    rows_key: Option<(bool, Sort)>,
    previews: PreviewCache,
    listing: Option<DirListing>,
    path_edit: String,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    error: Option<String>,
    loading: bool,
    places: Vec<Place>,
    rect: Option<Rect>,
    layer: Option<egui::LayerId>,
    viewport: Option<egui::ViewportId>,
}

impl Pane {
    fn set_listing(&mut self, l: DirListing) {
        self.previews.clear();
        self.rows_key = None;
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
    pub progress_open: bool,
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

impl Drop for FileManager {
    fn drop(&mut self) {for job in &self.jobs {if job.is_active() {job.cancel();}}}
}

impl FileManager {
    pub fn new(fs: RemoteFs, remote_name: String) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            open: false,
            progress_open: false,
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
        let viewport=ctx.viewport_id();
        rt.spawn(async move {
            let r = fs.list(path).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::RemoteListing(r));
            ctx.request_repaint_of(viewport);ctx.request_repaint_of(egui::ViewportId::ROOT);
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
        let viewport=ctx.viewport_id();
        rt.spawn(async move {
            if let Ok(p) = fs.places().await {
                let _ = tx.send(Msg::RemotePlaces(p));
                ctx.request_repaint_of(viewport);ctx.request_repaint_of(egui::ViewportId::ROOT);
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
            destination: Mutex::new(None),
            upload: true,
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
            state: Mutex::new(JobState::Preparing),
            task: Mutex::new(None),
        });
        let (fs, tx, ctx2, j) = (self.fs.clone(), self.tx.clone(), ctx.clone(), job.clone());
        let viewport=ctx.viewport_id();
        let task = rt.spawn(async move {
            let result: anyhow::Result<String> = async {
                let target = match point {
                    Some((x,y)) => fs.drop_target(x,y).await?,
                    None => target,
                };
                *j.destination.lock().unwrap() = Some(match &target {
                    UploadTarget::Folder(path)=>path.clone(),
                    UploadTarget::Default=>"Remote Desktop".into(),
                    UploadTarget::UnderCursor=>"Remote folder under the pointer".into(),
                    UploadTarget::Clipboard {..}=>"Remote clipboard".into(),
                });
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
                anyhow::ensure!(j.begin(),"Transfer was cancelled before sending files");
                let mut last = String::new();
                for (path, rel, _) in &files {
                    last = fs
                        .upload(path, target.clone(), rel.clone(), |n| {
                            j.done.fetch_add(n, Ordering::Relaxed);
                            ctx2.request_repaint_after_for(std::time::Duration::from_millis(100),viewport);
                            ctx2.request_repaint_after_for(std::time::Duration::from_millis(100),egui::ViewportId::ROOT);
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
            j.finish(match result {
                Ok(m) => JobState::Done(m),
                Err(e) => JobState::Failed(format!("{e:#}")),
            });
            let _ = tx.send(Msg::JobFinished { upload: true });
            ctx2.request_repaint_of(viewport);ctx2.request_repaint_of(egui::ViewportId::ROOT);
        });
        *job.task.lock().unwrap() = Some(task);
        self.jobs.push(job);
        self.progress_open = true;
        ctx.request_repaint_of(egui::ViewportId::ROOT);
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
            destination: Mutex::new(Some(dest_dir.display().to_string())),
            upload: false,
            total: AtomicU64::new(0),
            done: AtomicU64::new(0),
            state: Mutex::new(JobState::Preparing),
            task: Mutex::new(None),
        });
        let (fs, tx, ctx2, j) = (self.fs.clone(), self.tx.clone(), ctx.clone(), job.clone());
        let viewport=ctx.viewport_id();
        let task = rt.spawn(async move {
            let result: anyhow::Result<String> = async {
                let mut files = Vec::new();
                for (path, is_dir, name, size) in picks {
                    files.extend(fs.walk(path, is_dir, name, size).await?);
                }
                j.total.store(files.iter().map(|f| f.2).sum(), Ordering::Relaxed);
                anyhow::ensure!(j.begin(),"Transfer was cancelled before sending files");
                let mut last = PathBuf::new();
                for (remote, rel, _) in &files {
                    let dest = dest_dir.join(fsops::safe_relative(rel)?);
                    last = fs
                        .download(remote.clone(), &dest, |n| {
                            j.done.fetch_add(n, Ordering::Relaxed);
                            ctx2.request_repaint_after_for(std::time::Duration::from_millis(100),viewport);
                            ctx2.request_repaint_after_for(std::time::Duration::from_millis(100),egui::ViewportId::ROOT);
                        })
                        .await?;
                }
                Ok(match files.len() {
                    1 => format!("saved to {}", last.display()),
                    n => format!("{n} files received"),
                })
            }
            .await;
            j.finish(match result {
                Ok(m) => JobState::Done(m),
                Err(e) => JobState::Failed(format!("{e:#}")),
            });
            let _ = tx.send(Msg::JobFinished { upload: false });
            ctx2.request_repaint_of(viewport);ctx2.request_repaint_of(egui::ViewportId::ROOT);
        });
        *job.task.lock().unwrap() = Some(task);
        self.jobs.push(job);
        self.progress_open = true;
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn handle_messages(&mut self, rt: &tokio::runtime::Runtime, ctx: &egui::Context) {
        self.local.previews.poll(ctx);
        self.remote.previews.poll(ctx);
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
        (self.open && self.remote.viewport==Some(ctx.viewport_id()) && self.remote.rect.is_some_and(|r| r.contains(pos))
            && ctx.layer_id_at(pos).is_some_and(|layer| Some(layer) == self.remote.layer))
            .then(|| self.remote.listing.as_ref().map(|l| UploadTarget::Folder(l.path.clone())))
            .flatten()
    }

    /// Independent desktop window; mobile backends embed the same body.
    pub fn show(shared:&Arc<Mutex<Self>>,ctx:&egui::Context,rt:&Arc<tokio::runtime::Runtime>,id:u64) {
        Self::show_progress(shared,ctx,rt,id);
        let host={
            let mut files=shared.lock().expect("file window");
            files.handle_messages(rt,ctx);
            if !files.open {return;}
            files.remote_name.clone()
        };
        let (shared,rt)=(shared.clone(),rt.clone());
        let viewport=egui::ViewportId::from_hash_of(("file-transfer",id));
        let builder=egui::ViewportBuilder::default().with_decorations(false)
            .with_transparent(theme::transparent_window()).with_app_id("slipstream-remote")
            .with_title(format!("File transfer · {host} · Slipstream Remote"))
            .with_icon(Arc::new(theme::icon(64)))
            .with_inner_size([1100.0,740.0]).with_min_inner_size([720.0,400.0]);
        ctx.show_viewport_deferred(viewport,builder,move |ui,class| {
            let mut files=shared.lock().expect("file window");
            let ctx=ui.ctx().clone();
            let connected=files.fs.is_connected();
            if !files.open {
                if class==egui::ViewportClass::Deferred {ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
                return;
            }
            files.handle_messages(&rt,&ctx);
            let desktop=class==egui::ViewportClass::Deferred;
            let (body,close)=theme::tool_frame(ui,&format!("File transfer · {host}"),desktop,pal::ORANGE);
            if close || ui.input(|i|i.viewport().close_requested()) {
                files.open=false;
                if desktop {ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
                ctx.request_repaint_of(egui::ViewportId::ROOT);return;
            }
            let ui=&mut ui.new_child(egui::UiBuilder::new().max_rect(body));
            if connected {files.ensure_started(&rt,&ctx);} else {
                ui.label(RichText::new("Remote host unavailable · reconnect to continue").color(pal::GOLD));
            }
            ui.add_enabled_ui(connected,|ui|files.contents(ui,&rt));
            let dropped=ctx.input(|i|i.raw.dropped_files.iter().map(|f|f.path().to_path_buf()).collect::<Vec<_>>());
            let hovering=ctx.input(|i|!i.raw.hovered_files.is_empty());
            let target=ctx.pointer_hover_pos().and_then(|p|files.remote_drop_target(p,&ctx));
            if connected && hovering && target.is_some()
                && let Some(rect)=files.remote.rect {theme::brackets(ui.painter(),rect,pal::ORANGE,18.0);}
            if !dropped.is_empty() {
                if connected && let Some(target)=target {files.upload(dropped,target,&rt,&ctx);}
                else {files.remote.error=Some("Drop files or folders onto the remote pane while connected".into());}
            }
            // The parent may only run logic while minimized. Keep this
            // viewport responsive to transport/job changes on its own.
            ctx.request_repaint_after(std::time::Duration::from_millis(if files.activity().is_some() {100}else{250}));
        });
    }

    /// A small native receipt window for drops, separate from the browser.
    /// Completion stays visible until dismissed; closing never cancels work.
    pub(super) fn show_progress(shared:&Arc<Mutex<Self>>,ctx:&egui::Context,rt:&Arc<tokio::runtime::Runtime>,id:u64) {
        let host={
            let files=shared.lock().expect("transfer progress");
            if !files.progress_open {return;}
            files.remote_name.clone()
        };
        let (shared,rt)=(shared.clone(),rt.clone());
        let viewport=egui::ViewportId::from_hash_of(("transfer-progress",id));
        let builder=egui::ViewportBuilder::default().with_decorations(false)
            .with_transparent(theme::transparent_window()).with_app_id("slipstream-remote")
            .with_title(format!("Transfer progress · {host} · Slipstream Remote")).with_icon(Arc::new(theme::icon(64)))
            .with_inner_size([520.0,360.0]).with_min_inner_size([360.0,220.0]);
        ctx.show_viewport_deferred(viewport,builder,move |ui,class| {
            let mut files=shared.lock().expect("transfer progress");
            let ctx=ui.ctx().clone();
            let desktop=class==egui::ViewportClass::Deferred;
            if !files.progress_open {
                if desktop {ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
                return;
            }
            files.handle_messages(&rt,&ctx);
            let (body,close)=theme::tool_frame(ui,&format!("Transfers · {host}"),desktop,pal::ORANGE);
            if close || ctx.input(|i|i.viewport().close_requested()) {
                files.progress_open=false;
                if desktop {ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
                ctx.request_repaint_of(egui::ViewportId::ROOT);return;
            }
            let ui=&mut ui.new_child(egui::UiBuilder::new().max_rect(body));
            ui.horizontal(|ui| {
                if theme::pill(ui,"Files",pal::BLUE).on_hover_text("Open the full file browser and transfer history").clicked() {
                    files.open=true;ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
                if ui.button("Clear finished").clicked() {
                    files.jobs.retain(|job|job.is_active());
                    if files.jobs.is_empty() {files.progress_open=false;}
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt(("transfer-cards",id)).auto_shrink([false,false])
                .max_height(ui.available_height()).show(ui,|ui| {
                for job in files.jobs.iter().rev() {
                    let state=job.state.lock().unwrap().clone();
                    let total=job.total.load(Ordering::Relaxed);
                    let done=job.done.load(Ordering::Relaxed);
                    let (status,color)=match &state {
                        JobState::Preparing=>("PREPARING",pal::GOLD),
                        JobState::Running if total>0 && done>=total=>("CONFIRMING",pal::GOLD),
                        JobState::Running=>(if job.upload {"SENDING"}else{"RECEIVING"},pal::BLUE),
                        JobState::Done(_)=>("COMPLETE",pal::GREEN),
                        JobState::Failed(_)=>("FAILED",pal::RED),
                        JobState::Cancelled=>("CANCELLED",pal::DIM),
                    };
                    egui::Frame::new().fill(pal::BG).stroke(egui::Stroke::new(1.0,color.gamma_multiply(0.5)))
                        .corner_radius(8.0).inner_margin(10.0).show(ui,|ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (icon,_)=ui.allocate_exact_size(vec2(20.0,20.0),Sense::hover());
                            entry_icon(ui.painter(),icon,false);
                            ui.add(egui::Label::new(RichText::new(&job.label).font(theme::body(16.0)).color(pal::TEXT)).truncate());
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(status).font(theme::display(16.0)).color(color));
                            if matches!(state,JobState::Preparing|JobState::Running) && ui.small_button("Cancel").clicked() {job.cancel();}
                        });
                        if let Some(destination)=job.destination.lock().unwrap().as_ref() {
                            ui.add(egui::Label::new(RichText::new(format!("To: {destination}")).font(theme::mono(12.0)).color(pal::DIM)).truncate()).on_hover_text(destination);
                        }
                        match &state {
                            JobState::Preparing=>{ui.label(RichText::new("Resolving destination and preparing files…").small().color(pal::DIM));}
                            JobState::Running=>{
                                let fraction=if total==0 {0.0}else{done as f32/total as f32};
                                let text=format!("{} / {} · {:.0}%",human_size(done),human_size(total),fraction*100.0);
                                theme::progress(ui,fraction,ui.available_width(),pal::BLUE,&text);
                            }
                            JobState::Done(message)=>{ui.label(RichText::new(message).small().color(pal::GREEN));}
                            JobState::Failed(message)=>{ui.label(RichText::new(message).small().color(pal::RED));}
                            JobState::Cancelled=>{ui.label(RichText::new("Transfer cancelled").small().color(pal::DIM));}
                        }
                    });
                    ui.add_space(6.0);
                }
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(if files.activity().is_some() {100}else{250}));
        });
    }

    pub fn disconnect(&mut self) {
        self.local.previews.clear();self.remote.previews.clear();
        for job in &self.jobs {
            job.stop(JobState::Failed("Link interrupted; restart this transfer after reconnecting".into()));
        }
    }

    pub fn reconnect(&mut self,fs:RemoteFs,name:String) {
        self.disconnect();
        let sort=self.remote.sort;
        self.fs=fs;self.remote_name=name;self.remote=Pane {sort,..Default::default()};self.started=false;
        // Late listings from the old connection must not replace the new pane.
        (self.tx,self.rx)=mpsc::channel();
    }

    fn contents(&mut self,ui:&mut Ui,rt:&tokio::runtime::Runtime) {
        if !self.jobs.is_empty() {
            egui::Panel::bottom("file-jobs").frame(egui::Frame::NONE).show(ui,|ui|self.jobs_ui(ui));
        }
        egui::CentralPanel::no_frame().show(ui,|ui|self.panes(ui,rt));
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
                go_local = pane_ui(ui, &mut self.local, "This computer", pal::BLUE, "local", self.show_hidden, &Loader {rt,remote:None});
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
                go_remote = pane_ui(ui, &mut self.remote, &title, pal::ORANGE, "remote", self.show_hidden, &Loader {rt,remote:Some(&self.fs)});
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

/// Console-style vector symbols, independent of the operating system's
/// emoji font. Shapes stay crisp at the current display scale.
pub(super) fn entry_icon(p:&egui::Painter,rect:Rect,folder:bool) {
    let at=|x,y|rect.min+vec2(x,y);
    let color=if folder {pal::GOLD}else{pal::BLUE};
    let stroke=egui::Stroke::new(1.4,color);
    if folder {
        p.rect_filled(Rect::from_min_max(at(1.0,2.0),at(10.0,8.0)),egui::CornerRadius{nw:3,ne:3,..Default::default()},color);
        let body=Rect::from_min_max(at(1.0,6.0),at(20.0,18.0));
        p.rect_filled(body,3.0,color.gamma_multiply(0.14));
        p.rect_stroke(body,3.0,stroke,egui::StrokeKind::Inside);
        p.rect_filled(Rect::from_min_max(at(1.0,6.0),at(5.0,18.0)),egui::CornerRadius{nw:3,sw:3,..Default::default()},color);
        p.line_segment([at(5.0,8.0),at(18.0,8.0)],egui::Stroke::new(2.0,color));
        p.line_segment([at(11.0,14.0),at(17.0,14.0)],stroke);
    } else {
        let body=Rect::from_min_max(at(3.0,1.0),at(17.0,19.0));
        p.rect_filled(body,2.0,color.gamma_multiply(0.14));
        p.rect_stroke(body,2.0,stroke,egui::StrokeKind::Inside);
        p.rect_filled(Rect::from_min_max(at(3.0,1.0),at(6.0,19.0)),egui::CornerRadius{nw:2,sw:2,..Default::default()},color);
        p.line_segment([at(11.0,2.0),at(11.0,6.0)],stroke);
        p.line_segment([at(11.0,6.0),at(16.0,6.0)],stroke);
        for (y,end) in [(10.0,14.0),(13.0,14.0),(16.0,12.0)] {
            p.line_segment([at(8.0,y),at(end,y)],egui::Stroke::new(1.0,color));
        }
    }
}

/// Draws one pane. Returns `Some(path)` to navigate (`Some(None)` = home).
#[allow(clippy::too_many_arguments)]
fn pane_ui(ui: &mut Ui, pane: &mut Pane, title: &str, color: Color32, id: &str, show_hidden: bool, loader: &Loader<'_>) -> Option<Option<String>> {
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
    ui.push_id(("sort",id),|ui|ui.horizontal(|ui| {
        ui.label(RichText::new("SORT").font(theme::mono(11.0)).color(pal::DIM));
        for (field,label) in [(SortField::Name,"Name"),(SortField::Modified,"Modified")] {
            let active=pane.sort.field == field;
            let label=if active {format!("{label} {}",if pane.sort.descending {"▼"}else{"▲"})}else{label.into()};
            let button=egui::Button::new(RichText::new(label).color(if active {pal::BG}else{pal::TEXT}))
                .fill(if active {color}else{Color32::from_rgb(18,22,38)}).corner_radius(6.0);
            if ui.add(button).on_hover_text("Click to sort; click again to reverse. Folders stay first.").clicked() {
                pane.sort.choose(field);
            }
        }
    }));
    if let Some(e) = &pane.error {
        ui.label(RichText::new(e).color(pal::RED));
    }
    if pane.rows_key != Some((show_hidden,pane.sort)) {
        pane.rows=pane.listing.as_ref().map(|l|sorted_rows(&l.entries,show_hidden,pane.sort)).unwrap_or_default();
        pane.rows_key=Some((show_hidden,pane.sort));
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
        let row_h = 36.0;
        let modifiers = ui.input(|i| i.modifiers);
        let rows = &pane.rows;
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
                    let icon_rect=Rect::from_min_size(rect.left_center()+vec2(3.0,-16.0),vec2(32.0,32.0));
                    let previewable=!e.is_dir && slipstream_session::preview::supported(&e.name);
                    let path=format!("{}{}{}",listing.path.trim_end_matches(['/', '\\']),listing.sep,e.name);
                    if !previewable || !pane.previews.paint(ui,icon_rect,i,path,loader) {
                        entry_icon(p,Rect::from_center_size(icon_rect.center(),vec2(20.0,20.0)),e.is_dir);
                    }
                    // Two text lines keep long dates clear of file sizes, even in narrow panes.
                    let name_w = (rect.width() - 46.0).max(60.0);
                    let galley = p.layout(e.name.clone(), theme::body(15.0), if e.is_dir { pal::TAN } else { pal::TEXT }, name_w);
                    let name_rect = Rect::from_min_size(rect.left_top() + vec2(42.0, 0.0), vec2(name_w, 18.0));
                    p.with_clip_rect(name_rect).galley(name_rect.min, galley, pal::TEXT);
                    let details=Rect::from_min_max(rect.min+vec2(42.0,19.0),rect.max);
                    if !e.is_dir {
                        p.with_clip_rect(details).text(details.left_top(),Align2::LEFT_TOP,human_size(e.size),theme::mono(11.0),pal::BLUE);
                    }
                    p.with_clip_rect(details).text(details.right_top()-vec2(4.0,0.0),Align2::RIGHT_TOP,short_date(e.modified),theme::mono(11.0),pal::DIM);
                    if previewable {resp.clone().on_hover_ui(|ui|pane.previews.hover(ui,i));}
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
    pane.viewport = Some(ui.ctx().viewport_id());
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
    fn sort_keeps_folders_first_unknown_dates_last_and_selection_identity() {
        let entries:Vec<_>=[("z-folder",true,20),("a-folder",true,10),("Alpha.txt",false,20),
            ("beta.txt",false,30),(".hidden",false,40),("unknown.txt",false,0),("alpha.txt",false,20)]
            .into_iter().map(|(name,is_dir,modified)|DirEntry {name:name.into(),is_dir,size:10,modified}).collect();
        let mut sort=Sort::default();
        assert_eq!(sorted_rows(&entries,false,sort),[1,0,2,6,3,5]);
        let selected:BTreeSet<usize>=[2,3].into();
        sort.choose(SortField::Name);
        assert_eq!(sorted_rows(&entries,false,sort),[0,1,5,3,6,2]);
        sort.choose(SortField::Modified); // first date click is newest first
        let rows=sorted_rows(&entries,false,sort);
        assert_eq!(rows,[0,1,3,2,6,5]);
        assert_eq!(rows.iter().filter(|i|selected.contains(i)).map(|&i|entries[i].name.as_str()).collect::<Vec<_>>(),["beta.txt","Alpha.txt"]);
        // Shift ranges use the new display order, while the anchor stays an entry index.
        let anchor=rows.iter().position(|&i|i==3).unwrap();
        assert_eq!(&rows[anchor..=4],[3,2,6]);
        sort.choose(SortField::Modified);
        assert_eq!(sorted_rows(&entries,false,sort),[1,0,2,6,3,5]);
        assert!(sorted_rows(&entries,true,sort).contains(&4));
        let mut pane=Pane {sort,..Default::default()};
        pane.set_listing(DirListing {path:"/test".into(),parent:None,sep:"/".into(),entries});
        assert!(pane.sort.field==SortField::Modified && !pane.sort.descending);
        assert!(pane.rows_key.is_none());
    }

    #[test]
    fn late_transfer_receipts_preserve_cancelled_or_disconnected_results() {
        let job=Job {label:"file.txt".into(),destination:Mutex::new(None),upload:true,total:AtomicU64::new(10),
            done:AtomicU64::new(10),state:Mutex::new(JobState::Running),task:Mutex::new(None)};
        // Sending every byte still leaves the job running until the receipt.
        assert!(job.is_active());
        job.cancel();assert!(!job.begin());job.finish(JobState::Done("saved remotely".into()));
        assert_eq!(*job.state.lock().unwrap(),JobState::Cancelled);
        *job.state.lock().unwrap()=JobState::Failed("Link interrupted".into());
        job.finish(JobState::Done("late reply".into()));
        assert_eq!(*job.state.lock().unwrap(),JobState::Failed("Link interrupted".into()));
        *job.state.lock().unwrap()=JobState::Running;
        job.finish(JobState::Done("saved remotely".into()));job.cancel();job.stop(JobState::Failed("late disconnect".into()));
        assert_eq!(*job.state.lock().unwrap(),JobState::Done("saved remotely".into()));
    }

    #[test]
    fn formats() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(short_date(0), "");
        assert_eq!(short_date(1_792_183_720), "2026-10-16 20:48");
    }
}
