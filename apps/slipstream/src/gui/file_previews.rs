//! Visible-row preview requests and a bounded texture cache for one file pane.
use std::{collections::HashMap, sync::mpsc};

use eframe::egui::{self, Color32, Rect, Ui, vec2};
use slipstream_core::proto::FilePreview;
use slipstream_session::RemoteFs;

use super::{
    files::entry_icon,
    theme::{self, pal},
};

pub(super) struct Loader<'a> {
    pub rt: &'a tokio::runtime::Runtime,
    pub remote: Option<&'a RemoteFs>,
}

enum Preview {
    Image(egui::TextureHandle),
    Text(String),
    Missing,
}

struct Loaded {
    generation: u64,
    index: usize,
    result: Result<Option<FilePreview>, ()>,
    remote: bool,
}

pub(super) struct PreviewCache {
    generation: u64,
    clock: u64,
    entries: HashMap<usize, (u64, Preview)>,
    tasks: HashMap<usize, tokio::task::JoinHandle<()>>,
    tx: mpsc::Sender<Loaded>,
    rx: mpsc::Receiver<Loaded>,
    remote_unavailable: bool,
}

impl Default for PreviewCache {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            generation: 0,
            clock: 0,
            entries: HashMap::new(),
            tasks: HashMap::new(),
            tx,
            rx,
            remote_unavailable: false,
        }
    }
}

impl Drop for PreviewCache {
    fn drop(&mut self) {
        self.clear();
    }
}

impl PreviewCache {
    /// A refreshed listing may have the same names/indices but new content.
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        for (_, task) in self.tasks.drain() {
            task.abort();
        }
        self.entries.clear();
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(loaded) = self.rx.try_recv() {
            if loaded.generation != self.generation {
                continue;
            }
            self.tasks.remove(&loaded.index);
            if loaded.remote && loaded.result.is_err() {
                // An older host may not implement the optional preview RPC.
                // Do not send one failing request for every file in a folder.
                self.remote_unavailable = true;
            }
            let preview = match loaded.result.ok().flatten().filter(FilePreview::valid) {
                Some(FilePreview::Image {
                    width,
                    height,
                    rgba,
                }) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    );
                    Preview::Image(ctx.load_texture(
                        format!("file-preview-{}-{}", self.generation, loaded.index),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                Some(FilePreview::Text(text)) => Preview::Text(text),
                _ => Preview::Missing,
            };
            if self.entries.len() >= 128
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (used, _))| used)
                    .map(|(&index, _)| index)
            {
                self.entries.remove(&oldest);
            }
            self.clock += 1;
            self.entries.insert(loaded.index, (self.clock, preview));
        }
    }

    /// Returns false while loading or for files with no passive preview.
    pub fn paint(
        &mut self,
        ui: &Ui,
        rect: Rect,
        index: usize,
        path: String,
        loader: &Loader<'_>,
    ) -> bool {
        if let Some((used, preview)) = self.entries.get_mut(&index) {
            self.clock += 1;
            *used = self.clock;
            let p = ui.painter();
            match preview {
                Preview::Image(texture) => {
                    p.rect_filled(rect, 3.0, Color32::from_rgb(22, 26, 38));
                    let size = texture.size_vec2();
                    let scale = (rect.width() / size.x).min(rect.height() / size.y);
                    p.image(
                        texture.id(),
                        Rect::from_center_size(rect.center(), size * scale),
                        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    p.rect_stroke(
                        rect,
                        3.0,
                        egui::Stroke::new(1.0, pal::BLUE.gamma_multiply(0.5)),
                        egui::StrokeKind::Inside,
                    );
                    return true;
                }
                Preview::Text(text) => {
                    entry_icon(
                        p,
                        Rect::from_center_size(rect.center(), vec2(20.0, 20.0)),
                        false,
                    );
                    // A little actual page content replaces the generic data lines.
                    let page = Rect::from_min_size(rect.min + vec2(11.0, 10.0), vec2(12.0, 13.0));
                    p.rect_filled(page, 0.0, Color32::from_rgb(4, 6, 12));
                    for (line, text) in text.lines().take(5).enumerate() {
                        let text: String = text.chars().take(8).collect();
                        p.with_clip_rect(page).text(
                            page.min + vec2(0.0, line as f32 * 2.5),
                            egui::Align2::LEFT_TOP,
                            text,
                            theme::mono(2.4),
                            pal::BLUE,
                        );
                    }
                    return true;
                }
                Preview::Missing => return false,
            }
        }
        if self.tasks.len() >= 2
            || self.tasks.contains_key(&index)
            || (loader.remote.is_some() && self.remote_unavailable)
        {
            return false;
        }
        let (tx, generation, remote, ctx) = (
            self.tx.clone(),
            self.generation,
            loader.remote.cloned(),
            ui.ctx().clone(),
        );
        let viewport = ctx.viewport_id();
        let task = loader.rt.spawn(async move {
            let is_remote = remote.is_some();
            let result = match remote {
                Some(fs) => fs.preview(path).await.map_err(|_| ()),
                None => Ok(slipstream_session::preview::load(path).await.ok().flatten()),
            };
            let _ = tx.send(Loaded {
                generation,
                index,
                result,
                remote: is_remote,
            });
            theme::repaint_existing_viewport(&ctx,viewport);
        });
        self.tasks.insert(index, task);
        false
    }

    pub fn hover(&self, ui: &mut Ui, index: usize) {
        if let Some((_, preview)) = self.entries.get(&index) {
            match preview {
                Preview::Image(texture) => {
                    ui.image((texture.id(), texture.size_vec2()));
                }
                Preview::Text(text) => {
                    let text = text
                        .lines()
                        .take(24)
                        .map(|line| line.chars().take(100).collect::<String>())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::mono(12.0))
                                .color(pal::TEXT),
                        )
                        .wrap(),
                    );
                    ui.label(
                        egui::RichText::new("Text preview · first 8 KB")
                            .small()
                            .color(pal::DIM),
                    );
                }
                Preview::Missing => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_results_are_ignored_and_cache_is_bounded() {
        let ctx = egui::Context::default();
        let mut cache = PreviewCache::default();
        cache
            .tx
            .send(Loaded {
                generation: 0,
                index: 0,
                result: Ok(Some(FilePreview::Text("old folder".into()))),
                remote: false,
            })
            .unwrap();
        cache.clear();
        cache.poll(&ctx);
        assert!(cache.entries.is_empty());
        for index in 0..140 {
            cache
                .tx
                .send(Loaded {
                    generation: cache.generation,
                    index,
                    result: Ok(Some(FilePreview::Text(format!("row {index}")))),
                    remote: false,
                })
                .unwrap();
        }
        cache.poll(&ctx);
        assert_eq!(cache.entries.len(), 128);
        assert!(!cache.entries.contains_key(&0));
        assert!(cache.entries.contains_key(&139));
        cache
            .tx
            .send(Loaded {
                generation: cache.generation,
                index: 200,
                result: Err(()),
                remote: true,
            })
            .unwrap();
        cache.poll(&ctx);
        assert!(cache.remote_unavailable);
        cache.clear();
        assert!(
            cache.remote_unavailable,
            "older-host fallback persists across folders"
        );
        // Invalid dimensions/lengths never reach egui's texture allocator.
        assert!(
            !FilePreview::Image {
                width: 0,
                height: 2,
                rgba: vec![]
            }
            .valid()
        );
        assert!(
            !FilePreview::Image {
                width: 128,
                height: 128,
                rgba: vec![0; 10]
            }
            .valid()
        );
        assert!(
            !FilePreview::Image {
                width: 129,
                height: 1,
                rgba: vec![0; 516]
            }
            .valid()
        );
    }
}
