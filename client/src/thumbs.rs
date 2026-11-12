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
    Ready {
        tex: TextureHandle,
        used: u64,
        tint: Option<[u8; 3]>,
    },
    Failed,
}

/// A decoded thumbnail and its dominant colour.
type Decoded = (ColorImage, Option<[u8; 3]>);
type Done = Arc<Mutex<Vec<(String, Option<Decoded>)>>>;

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
                Some((img, tint)) => Slot::Ready {
                    tex: ctx.load_texture(format!("thumb-{id}"), img, TextureOptions::LINEAR),
                    used: self.frame,
                    tint,
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
            Some(Slot::Ready { tex, used, .. }) => {
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

impl Thumbs {
    /// The thumbnail's dominant colour. `None` while it is still loading
    /// (which this starts), `Some(None)` when it has no clear colour.
    pub fn tint(&mut self, youtube_id: &str) -> Option<Option<[u8; 3]>> {
        self.get(youtube_id);
        match self.slots.get(youtube_id) {
            Some(Slot::Ready { tint, .. }) => Some(*tint),
            Some(Slot::Failed) => Some(None),
            _ => None,
        }
    }
}

const HUE_BINS: usize = 18;
/// Pixels greyer or darker than this say nothing about the picture's colour
/// (letterbox bars, shadows, white text).
const MIN_SAT: f32 = 0.25;
const MIN_VALUE: f32 = 0.2;
/// The winning hue must cover at least this share of the picture.
const MIN_SHARE: f32 = 0.04;

/// The colour a picture is "about": the average of its most common vivid
/// hue. `None` for grey or nearly colourless pictures.
pub fn dominant_color(img: &ColorImage) -> Option<[u8; 3]> {
    let mut weight = [0f32; HUE_BINS];
    let mut count = [0u32; HUE_BINS];
    let mut sum = [[0f32; 3]; HUE_BINS];
    for px in &img.pixels {
        let rgb = [px.r(), px.g(), px.b()];
        let hsva = egui::ecolor::Hsva::from_srgb(rgb);
        if hsva.s < MIN_SAT || hsva.v < MIN_VALUE {
            continue;
        }
        let bin = ((hsva.h * HUE_BINS as f32) as usize).min(HUE_BINS - 1);
        // Vivid pixels count for more than dull ones.
        let w = hsva.s * hsva.v;
        weight[bin] += w;
        count[bin] += 1;
        for (s, c) in sum[bin].iter_mut().zip(rgb) {
            *s += c as f32 * w;
        }
    }
    // A hue split across two neighbouring bins still wins.
    let score = |i: usize| {
        weight[i] + 0.5 * (weight[(i + 1) % HUE_BINS] + weight[(i + HUE_BINS - 1) % HUE_BINS])
    };
    let best = (0..HUE_BINS).max_by(|a, b| score(*a).total_cmp(&score(*b)))?;
    let around =
        count[best] + count[(best + 1) % HUE_BINS] + count[(best + HUE_BINS - 1) % HUE_BINS];
    if weight[best] <= 0.0 || (around as f32) < img.pixels.len() as f32 * MIN_SHARE {
        return None;
    }
    Some(sum[best].map(|s| (s / weight[best]).round() as u8))
}

fn fetch(agent: &ureq::Agent, id: &str) -> Result<Decoded, String> {
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
    let img = ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    let tint = dominant_color(&img);
    Ok((img, tint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    fn image(pixels: Vec<Color32>) -> ColorImage {
        ColorImage::new([pixels.len(), 1], pixels)
    }

    #[test]
    fn grey_pictures_have_no_colour() {
        let mut px = vec![Color32::from_gray(30); 500];
        px.extend(vec![Color32::from_gray(220); 500]);
        // A few coloured pixels (a logo in the corner) are not enough.
        px.extend(vec![Color32::from_rgb(255, 0, 0); 10]);
        assert_eq!(dominant_color(&image(px)), None);
    }

    #[test]
    fn finds_the_main_colour() {
        // Black bars, a lot of teal, a little red.
        let mut px = vec![Color32::BLACK; 300];
        px.extend(vec![Color32::from_rgb(20, 170, 160); 500]);
        px.extend(vec![Color32::from_rgb(230, 30, 30); 200]);
        let [r, g, b] = dominant_color(&image(px)).unwrap();
        assert!(g > r && b > r, "{r} {g} {b}");
    }
}
