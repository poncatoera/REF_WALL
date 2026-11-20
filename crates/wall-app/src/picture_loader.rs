//! Pictures on big boards: nothing is decoded when a library opens. Pictures
//! on screen first load as small previews (cached in the library's `cache/`
//! folder, so later opens are quick), and only pictures shown larger than
//! their preview load at full size. Full-size textures stay under a memory
//! budget: when it fills, those not on screen drop back to previews.
//!
//! On desktop the decoding runs on worker threads; on the web (no threads)
//! a couple of pictures load per frame.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use wall_format::{ContentHash, ItemKind, MediaKind};
use wall_render::Renderer;

use crate::{App, Picture};

/// Longest side of a preview, in pixels.
pub const PREVIEW_PX: u32 = 512;
/// Full-size textures may use this much (RGBA bytes, before mipmaps).
pub const FULL_BUDGET: usize = 512 << 20;
/// Decodes running at once.
const MAX_IN_FLIGHT: usize = 8;
/// Pictures loaded per frame where there are no threads (web).
#[cfg(target_arch = "wasm32")]
const PER_FRAME: usize = 2;

type Rgba = (Vec<u8>, u32, u32);

/// One picture to decode.
#[derive(Debug, Clone)]
pub struct Job {
    pub hash: ContentHash,
    pub media: PathBuf,
    /// Folder of cached previews (`cache/previews` in the library).
    pub cache_dir: PathBuf,
    /// Also return the full-size picture.
    pub full: bool,
}

/// What a job produced.
pub struct Done {
    pub hash: ContentHash,
    pub preview: Option<Rgba>,
    pub full: Option<Rgba>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Loader {
    in_flight: HashSet<ContentHash>,
    /// Full-size pictures on the GPU and their size in bytes.
    full: HashMap<ContentHash, usize>,
    /// Frame each picture was last on screen.
    seen: HashMap<ContentHash, u64>,
    frame: u64,
    #[cfg(not(target_arch = "wasm32"))]
    results: Option<(std::sync::mpsc::Sender<Done>, std::sync::mpsc::Receiver<Done>)>,
    #[cfg(target_arch = "wasm32")]
    queue: Vec<Job>,
}

impl Loader {
    /// Bytes of full-size pictures on the GPU.
    pub fn full_bytes(&self) -> usize {
        self.full.values().sum()
    }

    pub fn busy(&self) -> bool {
        !self.in_flight.is_empty()
    }

    fn start(&mut self, job: Job) {
        self.in_flight.insert(job.hash.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, _) = self.results.get_or_insert_with(std::sync::mpsc::channel);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(run_job(&job));
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.queue.push(job);
    }

    /// Finished jobs, without waiting.
    fn finished(&mut self) -> Vec<Done> {
        #[cfg(not(target_arch = "wasm32"))]
        let done: Vec<Done> = self.results.as_ref().map(|(_, rx)| rx.try_iter().collect()).unwrap_or_default();
        #[cfg(target_arch = "wasm32")]
        let done: Vec<Done> = {
            let n = self.queue.len().min(PER_FRAME);
            self.queue.drain(..n).map(|j| run_job(&j)).collect()
        };
        for d in &done {
            self.in_flight.remove(&d.hash);
        }
        done
    }
}

fn cached_preview(dir: &Path, hash: &ContentHash) -> Option<PathBuf> {
    ["jpg", "png"].into_iter().map(|e| dir.join(format!("{hash}.{e}"))).find(|p| p.is_file())
}

/// Decodes one picture: the cached preview when only a preview is wanted
/// and one exists, else the file itself (writing the preview cache).
pub fn run_job(job: &Job) -> Done {
    let mut done = Done { hash: job.hash.clone(), preview: None, full: None, error: None };
    if !job.full {
        let cached = cached_preview(&job.cache_dir, &job.hash).and_then(|p| std::fs::read(p).ok()).and_then(|b| image::load_from_memory(&b).ok());
        if let Some(img) = cached {
            let img = img.to_rgba8();
            let (w, h) = img.dimensions();
            done.preview = Some((img.into_raw(), w, h));
            return done;
        }
    }
    let img = match std::fs::read(&job.media).map_err(|e| e.to_string()).and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string())) {
        Ok(img) => img,
        Err(e) => {
            done.error = Some(e);
            return done;
        }
    };
    let alpha = img.color().has_alpha();
    let small = img.thumbnail(PREVIEW_PX, PREVIEW_PX);
    if cached_preview(&job.cache_dir, &job.hash).is_none() {
        // A cache write failing only costs speed next time.
        let _ = std::fs::create_dir_all(&job.cache_dir);
        let _ = if alpha {
            small.save_with_format(job.cache_dir.join(format!("{}.png", job.hash)), image::ImageFormat::Png)
        } else {
            let path = job.cache_dir.join(format!("{}.jpg", job.hash));
            std::fs::File::create(&path)
                .map_err(image::ImageError::IoError)
                .and_then(|f| image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(f), 85).encode_image(&small.to_rgb8()))
        };
    }
    let small = small.to_rgba8();
    let (w, h) = small.dimensions();
    done.preview = Some((small.into_raw(), w, h));
    if job.full {
        let full = img.to_rgba8();
        let (w, h) = full.dimensions();
        done.full = Some((full.into_raw(), w, h));
    }
    done
}

impl App {
    /// Per frame, before drawing: uploads decoded pictures, starts loads for
    /// pictures on screen, and keeps full-size pictures within budget.
    pub(crate) fn pump_pictures(&mut self, renderer: &mut Renderer) {
        self.loader.frame += 1;
        let frame = self.loader.frame;

        // Imported pictures arrive decoded at full size.
        for (hash, pic) in self.pictures.iter_mut() {
            if let Picture::Pending { rgba, width, height } = pic {
                match renderer.create_image(rgba, *width, *height) {
                    Ok(id) => {
                        self.loader.full.insert(hash.clone(), rgba.len());
                        *pic = Picture::Ready(id);
                    }
                    Err(e) => eprintln!("wall: image upload failed: {e}"),
                }
            }
        }

        for done in self.loader.finished() {
            self.apply_loaded(done, renderer);
        }

        let Some(cache_dir) = self.library.as_ref().map(|l| l.root().join("cache").join("previews")) else { return };

        // What's on screen, largest first, and how many texture pixels wide
        // each is shown.
        let tl = self.camera.screen_to_board([0.0, 0.0], self.viewport);
        let br = self.camera.screen_to_board(self.viewport, self.viewport);
        let zoom = self.camera.zoom;
        let mut on_screen: Vec<(ContentHash, f32)> = self
            .doc
            .board
            .live_items()
            .filter(|i| {
                let f = i.frame;
                (f.x as f32) < br[0] && ((f.x + f.w) as f32) > tl[0] && (f.y as f32) < br[1] && ((f.y + f.h) as f32) > tl[1]
            })
            .filter_map(|i| match &i.kind {
                ItemKind::Media { hash, crop } => {
                    let cw = crop.map_or(1.0, |c| c.w).max(1e-3) as f32;
                    Some((hash.clone(), i.frame.w as f32 * zoom / cw))
                }
                _ => None,
            })
            .collect();
        on_screen.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (h, _) in &on_screen {
            self.loader.seen.insert(h.clone(), frame);
        }

        for (hash, px) in &on_screen {
            if self.loader.in_flight.len() >= MAX_IN_FLIGHT {
                break;
            }
            if self.loader.in_flight.contains(hash) || self.videos.contains_key(hash) {
                continue;
            }
            let Some(sc) = self.sidecars.get(hash).filter(|s| s.kind == MediaKind::Image) else { continue };
            // Full size only when shown clearly larger than the preview.
            let wants_full = *px > PREVIEW_PX as f32 * 1.1 && sc.width.max(sc.height) > PREVIEW_PX;
            let full = match self.pictures.get(hash) {
                Some(Picture::Unloaded) => wants_full,
                Some(Picture::Preview(_)) if wants_full => true,
                _ => continue,
            };
            let Some(lib) = &self.library else { break };
            let job = Job { hash: hash.clone(), media: lib.media_path(hash, &sc.ext), cache_dir: cache_dir.clone(), full };
            self.loader.start(job);
        }

        // Over budget: full-size pictures off screen longest go back to previews.
        if self.loader.full_bytes() > FULL_BUDGET {
            let mut fulls: Vec<(ContentHash, u64)> = self
                .loader
                .full
                .keys()
                .filter(|h| !self.videos.contains_key(*h))
                .map(|h| (h.clone(), self.loader.seen.get(h).copied().unwrap_or(0)))
                .filter(|(_, seen)| *seen < frame)
                .collect();
            fulls.sort_by_key(|(_, seen)| *seen);
            for (hash, _) in fulls {
                if self.loader.full_bytes() <= FULL_BUDGET {
                    break;
                }
                if let Some(Picture::Ready(id)) = self.pictures.get(&hash) {
                    renderer.delete_image(*id);
                }
                self.pictures.insert(hash.clone(), Picture::Unloaded);
                self.loader.full.remove(&hash);
            }
        }

        if self.loader.busy() {
            self.needs_redraw = true;
        }
    }

    fn apply_loaded(&mut self, done: Done, renderer: &mut Renderer) {
        let hash = done.hash;
        if let Some(e) = done.error {
            eprintln!("wall: could not load picture {hash}: {e}");
            return;
        }
        // Libraries from before auto-tags get them on first load.
        if let (Some(sc), Some((px, w, h))) = (self.sidecars.get_mut(&hash), &done.preview) {
            if sc.colors.is_empty() && sc.auto_tags.is_empty() {
                sc.colors = wall_media::autotag::dominant_colors(px, *w, *h, 4);
                crate::auto_tag(sc, None);
                if let Some(lib) = &self.library {
                    if let Err(e) = lib.write_sidecar(sc) {
                        eprintln!("wall: could not save auto-tags: {e}");
                    }
                }
            }
        }
        let old = self.pictures.get(&hash);
        let old_id = match old {
            Some(Picture::Preview(id) | Picture::Ready(id)) => Some(*id),
            _ => None,
        };
        let (rgba, w, h, full) = match (done.full, done.preview) {
            (Some((p, w, h)), _) => (p, w, h, true),
            (None, Some((p, w, h))) => {
                // Don't swap a full picture for a preview.
                if matches!(old, Some(Picture::Ready(_))) {
                    return;
                }
                (p, w, h, false)
            }
            (None, None) => return,
        };
        match renderer.create_image(&rgba, w, h) {
            Ok(id) => {
                if let Some(old) = old_id {
                    renderer.delete_image(old);
                }
                if full {
                    self.loader.full.insert(hash.clone(), rgba.len());
                    self.pictures.insert(hash, Picture::Ready(id));
                } else {
                    self.pictures.insert(hash, Picture::Preview(id));
                }
                self.needs_redraw = true;
            }
            Err(e) => eprintln!("wall: image upload failed: {e}"),
        }
    }

    /// Sets the view and cancels the fit-to-window an opened library would
    /// do on the first resize (WALL_CAMERA test hook).
    pub fn set_camera(&mut self, camera: crate::Camera) {
        self.camera = camera;
        self.fit_on_resize = false;
        self.needs_redraw = true;
    }

    /// Bytes of full-size pictures on the GPU (for tests and stats).
    pub fn full_picture_bytes(&self) -> usize {
        self.loader.full_bytes()
    }

    /// True while pictures are still loading.
    pub fn pictures_loading(&self) -> bool {
        self.loader.busy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        img.save_with_format(path, image::ImageFormat::Jpeg).unwrap();
    }

    #[test]
    fn job_writes_preview_cache_then_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("big.jpg");
        write_jpeg(&media, 2048, 1024);
        let hash = ContentHash::of(b"big");
        let cache_dir = dir.path().join("cache");
        let job = Job { hash: hash.clone(), media: media.clone(), cache_dir: cache_dir.clone(), full: false };

        let first = run_job(&job);
        let (_, w, h) = first.preview.unwrap();
        assert_eq!((w, h), (512, 256));
        assert!(first.full.is_none());
        let cached = cached_preview(&cache_dir, &hash).expect("preview cached");
        assert!(cached.extension().unwrap() == "jpg");

        // The second load comes from the cache, even with the file gone.
        std::fs::remove_file(&media).unwrap();
        let second = run_job(&job);
        assert_eq!(second.preview.unwrap().1, 512);

        // A full load needs the file.
        let full = run_job(&Job { full: true, ..job });
        assert!(full.error.is_some());
    }

    #[test]
    fn full_job_returns_both_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("big.jpg");
        write_jpeg(&media, 1600, 900);
        let done = run_job(&Job { hash: ContentHash::of(b"x"), media, cache_dir: dir.path().join("c"), full: true });
        assert_eq!(done.full.as_ref().map(|f| (f.1, f.2)), Some((1600, 900)));
        assert_eq!(done.preview.as_ref().map(|p| p.1), Some(512));
    }
}
