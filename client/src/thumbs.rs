//! YouTube thumbnails: fetched and decoded on worker threads, uploaded as
//! textures on the UI thread, least-recently-used ones evicted.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};

/// Decoded thumbnails are scaled to this size (16:9).
const W: u32 = 192;
const H: u32 = 108;
const WORKERS: usize = 3;
/// Max textures kept on the GPU (~80 KB each).
const MAX_TEXTURES: usize = 160;
const MAX_BYTES: u64 = 512 * 1024;

enum Slot {
    Loading,
    Ready { tex: TextureHandle, used: u64 },
    Failed,
}

type Done = Arc<Mutex<Vec<(String, Option<ColorImage>)>>>;

pub struct Thumbs {
    slots: HashMap<String, Slot>,
    jobs: mpsc::Sender<String>,
    done: Done,
    frame: u64,
}

impl Thumbs {
    pub fn new(ctx: egui::Context) -> Self {
        let (jobs, rx) = mpsc::channel::<String>();
        let rx = Arc::new(Mutex::new(rx));
        let done: Done = Arc::default();
        for i in 0..WORKERS {
            let rx = rx.clone();
            let done = done.clone();
            let ctx = ctx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("thumbs-{i}"))
                .spawn(move || {
                    let agent = ureq::Agent::config_builder()
                        .timeout_global(Some(Duration::from_secs(10)))
                        .user_agent("SyncTube")
                        .build()
                        .new_agent();
                    loop {
                        let job = rx.lock().unwrap().recv();
                        let Ok(id) = job else { return };
                        let img = fetch(&agent, &id);
                        if let Err(e) = &img {
                            tracing::debug!(id, "thumbnail: {e}");
                        }
                        done.lock().unwrap().push((id, img.ok()));
                        ctx.request_repaint();
                    }
                });
        }
        Self {
            slots: HashMap::new(),
            jobs,
            done,
            frame: 0,
        }
    }

    /// Call once per frame before [`Self::get`].
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        let finished = std::mem::take(&mut *self.done.lock().unwrap());
        for (id, img) in finished {
            let slot = match img {
                Some(img) => Slot::Ready {
                    tex: ctx.load_texture(format!("thumb-{id}"), img, TextureOptions::LINEAR),
                    used: self.frame,
                },
                None => Slot::Failed,
            };
            self.slots.insert(id, slot);
        }
        let ready = self
            .slots
            .values()
            .filter(|s| matches!(s, Slot::Ready { .. }))
            .count();
        if ready > MAX_TEXTURES {
            let mut by_age: Vec<(u64, String)> = self
                .slots
                .iter()
                .filter_map(|(k, s)| match s {
                    Slot::Ready { used, .. } => Some((*used, k.clone())),
                    _ => None,
                })
                .collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(ready - MAX_TEXTURES) {
                self.slots.remove(&k);
            }
        }
    }

    /// Texture for a video's thumbnail, if loaded. Starts loading it otherwise.
    pub fn get(&mut self, youtube_id: &str) -> Option<TextureId> {
        match self.slots.get_mut(youtube_id) {
            Some(Slot::Ready { tex, used }) => {
                *used = self.frame;
                Some(tex.id())
            }
            Some(_) => None,
            None => {
                if self.jobs.send(youtube_id.to_string()).is_ok() {
                    self.slots.insert(youtube_id.to_string(), Slot::Loading);
                } else {
                    self.slots.insert(youtube_id.to_string(), Slot::Failed);
                }
                None
            }
        }
    }
}

fn fetch(agent: &ureq::Agent, id: &str) -> Result<ColorImage, String> {
    if !shared::youtube::is_valid_video_id(id) {
        return Err("invalid id".into());
    }
    let url = format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg");
    let bytes = agent
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?
        .resize_to_fill(W, H, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    ))
}
