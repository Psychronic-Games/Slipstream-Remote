//! Local Wayland approval, independent of incoming connections and window visibility.
use std::sync::Arc;

use eframe::egui;
use slipstream_session::Client;
use tokio::sync::oneshot;

#[derive(Default)]
pub(super) struct ScreenAccess {
    job: Option<oneshot::Receiver<Result<bool, String>>>,
    task: Option<tokio::task::AbortHandle>,
    pub result: Option<Result<bool, String>>,
}

impl ScreenAccess {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn start(&mut self, rt: &Arc<tokio::runtime::Runtime>, client: &Client, ctx: &egui::Context, reset: bool) {
        if self.busy() {
            return;
        }
        let (tx, rx) = oneshot::channel();
        self.job = Some(rx);
        self.result = None;
        let (client, ctx) = (client.clone(), ctx.clone());
        let task = rt.spawn(async move {
            let result = async {
                if reset {
                    client.reset_screen_access().await?;
                }
                client.prepare_screen_access().await
            }.await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
        self.task = Some(task.abort_handle());
    }

    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() { task.abort(); }
        self.job = None;
        self.result = Some(Err("Screen approval cancelled; choose Approve now to try again".into()));
    }

    pub fn poll(&mut self) {
        let Some(job) = &mut self.job else { return };
        match job.try_recv() {
            Ok(result) => {
                self.result = Some(result);
                self.job = None;
                self.task = None;
            }
            Err(oneshot::error::TryRecvError::Closed) => {
                self.result = Some(Err("Screen approval ended; choose Approve now to try again".into()));
                self.job = None;
                self.task = None;
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
    }
}
