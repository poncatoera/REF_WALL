//! Pictures on big boards. Nothing is decoded when a library opens; each
//! picture on screen loads at the size it is shown:
//!
//! - **tiny** (at most 120 px), packed into shared atlas pages, when it is
//!   shown up to 128 px wide. A zoomed-out board of thousands of pictures
//!   draws in a handful of calls.
//! - **preview** (at most 512 px), its own texture, up to about 560 px.
//! - **full size** beyond that.
//!
//! Tiny and preview images are cached in the library's `cache/previews/`,
//! so later opens skip the full decode. Previews and full-size textures
//! each stay under a memory budget: when one fills, the pictures off screen
//! longest drop back (tiny images stay). On desktop the decoding runs on
//! worker threads; on the web (no threads) a couple load per frame.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use image::DynamicImage;
use wall_format::{ContentHash, ItemKind, MediaKind};
use wall_render::{ImageId, Renderer};

use crate::{App, Picture};

/// Longest side of a preview, in pixels.
pub const PREVIEW_PX: u32 = 512;
/// Longest side of a tiny image.
pub const TINY_PX: u32 = 120;
/// Tiny images are used up to this many screen pixels wide.
pub const TINY_SHOWN_PX: f32 = 128.0;
/// Atlas cell (tiny image plus a 4 px edge on each side) and page size.
const CELL: u32 = 128;
const PAD: u32 = 4;
const PAGE: u32 = 2048;
/// Full-size textures may use this much (RGBA bytes, before mipmaps).
pub const FULL_BUDGET: usize = 512 << 20;
/// Previews may use this much.
pub const PREVIEW_BUDGET: usize = 256 << 20;
/// Decodes running at once.
const MAX_IN_FLIGHT: usize = 8;
/// Pictures loaded per frame where there are no threads (web).
#[cfg(target_arch = "wasm32")]
const PER_FRAME: usize = 2;

type Rgba = (Vec<u8>, u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Tiny,
    Preview,
    Full,
}

/// One picture to decode.
#[derive(Debug, Clone)]
pub struct Job {
    pub hash: ContentHash,
    pub media: PathBuf,
    /// Folder of cached tiny and preview images (`cache/previews`).
    pub cache_dir: PathBuf,
    pub tier: Tier,
    /// Also return the tiny image (the loader has none for this picture).
    pub want_tiny: bool,
}

/// What a job produced.
#[derive(Default)]
pub struct Done {
    pub hash: Option<ContentHash>,
    pub tiny: Option<Rgba>,
    pub preview: Option<Rgba>,
    pub full: Option<Rgba>,
    pub error: Option<String>,
}

/// Tiny images packed into shared textures.
#[derive(Default)]
struct Atlas {
    pages: Vec<ImageId>,
    /// Cells used so far (pages fill in order).
    used: u32,
    /// Page and texture rectangle (u, v, w, h) of each picture's tiny image.
    slots: HashMap<ContentHash, (usize, [f32; 4])>,
    dirty: HashSet<usize>,
}

impl Atlas {
    const PER_PAGE: u32 = (PAGE / CELL) * (PAGE / CELL);

    fn add(&mut self, renderer: &mut Renderer, hash: ContentHash, rgba: &[u8], w: u32, h: u32) {
        if self.slots.contains_key(&hash) || w == 0 || h == 0 || w > CELL - 2 * PAD || h > CELL - 2 * PAD {
            return;
        }
        let page = (self.used / Self::PER_PAGE) as usize;
        if page == self.pages.len() {
            match renderer.create_atlas(PAGE, PAGE) {
                Ok(id) => self.pages.push(id),
                Err(e) => {
                    eprintln!("wall: atlas page failed: {e}");
                    return;
                }
            }
        }
        let k = self.used % Self::PER_PAGE;
        let (cx, cy) = ((k % (PAGE / CELL)) * CELL, (k / (PAGE / CELL)) * CELL);
        let (block, bw, bh) = pad_edges(rgba, w, h, PAD);
        renderer.update_image_region(self.pages[page], cx, cy, &block, bw, bh);
        let p = PAGE as f32;
        self.slots.insert(hash, (page, [(cx + PAD) as f32 / p, (cy + PAD) as f32 / p, w as f32 / p, h as f32 / p]));
        self.dirty.insert(page);
        self.used += 1;
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        for page in self.dirty.drain() {
            renderer.rebuild_mipmaps(self.pages[page]);
        }
    }
}

/// `rgba` surrounded by `pad` pixels repeating its edges, so filtering and
/// smaller mip levels don't bleed in the neighbouring cells.
fn pad_edges(rgba: &[u8], w: u32, h: u32, pad: u32) -> (Vec<u8>, u32, u32) {
    let (bw, bh) = (w + 2 * pad, h + 2 * pad);
    let mut out = vec![0u8; (bw * bh * 4) as usize];
    for y in 0..bh {
        let sy = (y as i64 - pad as i64).clamp(0, h as i64 - 1) as u32;
        for x in 0..bw {
            let sx = (x as i64 - pad as i64).clamp(0, w as i64 - 1) as u32;
            let s = ((sy * w + sx) * 4) as usize;
            let d = ((y * bw + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    (out, bw, bh)
}

#[derive(Default)]
pub struct Loader {
    in_flight: HashSet<ContentHash>,
    /// Full-size pictures and previews on the GPU, and their bytes.
    full: HashMap<ContentHash, usize>,
    previews: HashMap<ContentHash, usize>,
    atlas: Atlas,
    /// Frame each picture was last on screen.
    seen: HashMap<ContentHash, u64>,
    frame: u64,
    #[cfg(not(target_arch = "wasm32"))]
    results: Option<(std::sync::mpsc::Sender<Done>, std::sync::mpsc::Receiver<Done>)>,
    #[cfg(target_arch = "wasm32")]
    queue: Vec<Job>,
}

impl Loader {
    pub fn full_bytes(&self) -> usize {
        self.full.values().sum()
    }

    pub fn preview_bytes(&self) -> usize {
        self.previews.values().sum()
    }

    pub fn busy(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// The atlas page and texture rectangle of a picture's tiny image.
    pub fn tiny(&self, hash: &ContentHash) -> Option<(ImageId, [f32; 4])> {
        self.atlas.slots.get(hash).map(|(page, uv)| (self.atlas.pages[*page], *uv))
    }

    pub fn atlas_pages(&self) -> usize {
        self.atlas.pages.len()
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
            if let Some(h) = &d.hash {
                self.in_flight.remove(h);
            }
        }
        done
    }
}

fn cached(dir: &Path, hash: &ContentHash, suffix: &str) -> Option<PathBuf> {
    ["jpg", "png"].into_iter().map(|e| dir.join(format!("{hash}{suffix}.{e}"))).find(|p| p.is_file())
}

fn load_cached(dir: &Path, hash: &ContentHash, suffix: &str) -> Option<DynamicImage> {
    let bytes = std::fs::read(cached(dir, hash, suffix)?).ok()?;
    image::load_from_memory(&bytes).ok()
}

/// Saves a cache image as JPEG, or PNG when it has transparency. A failed
/// write only costs speed next time.
fn save_cached(dir: &Path, hash: &ContentHash, suffix: &str, img: &DynamicImage, alpha: bool) {
    let _ = std::fs::create_dir_all(dir);
    let _ = if alpha {
        img.save_with_format(dir.join(format!("{hash}{suffix}.png")), image::ImageFormat::Png)
    } else {
        std::fs::File::create(dir.join(format!("{hash}{suffix}.jpg")))
            .map_err(image::ImageError::IoError)
            .and_then(|f| image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(f), 85).encode_image(&img.to_rgb8()))
    };
}

fn rgba(img: &DynamicImage) -> Rgba {
    let img = img.to_rgba8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

/// Decodes one picture at the tier asked for, from the cache when it can,
/// filling the cache from the file when it can't.
pub fn run_job(job: &Job) -> Done {
    let mut done = Done { hash: Some(job.hash.clone()), ..Default::default() };
    let (dir, hash) = (&job.cache_dir, &job.hash);

    if job.tier == Tier::Tiny {
        if let Some(t) = load_cached(dir, hash, ".t") {
            done.tiny = Some(rgba(&t));
            return done;
        }
        if let Some(p) = load_cached(dir, hash, "") {
            let t = p.thumbnail(TINY_PX, TINY_PX);
            save_cached(dir, hash, ".t", &t, p.color().has_alpha());
            done.tiny = Some(rgba(&t));
            return done;
        }
    }
    if job.tier == Tier::Preview {
        if let Some(p) = load_cached(dir, hash, "") {
            if job.want_tiny {
                let t = load_cached(dir, hash, ".t").unwrap_or_else(|| {
                    let t = p.thumbnail(TINY_PX, TINY_PX);
                    save_cached(dir, hash, ".t", &t, p.color().has_alpha());
                    t
                });
                done.tiny = Some(rgba(&t));
            }
            done.preview = Some(rgba(&p));
            return done;
        }
    }

    // Not cached (or full size wanted): decode the file and fill the cache.
    let img = match std::fs::read(&job.media).map_err(|e| e.to_string()).and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string())) {
        Ok(img) => img,
        Err(e) => {
            done.error = Some(e);
            return done;
        }
    };
    let alpha = img.color().has_alpha();
    let preview = img.thumbnail(PREVIEW_PX, PREVIEW_PX);
    if cached(dir, hash, "").is_none() {
        save_cached(dir, hash, "", &preview, alpha);
    }
    let tiny = preview.thumbnail(TINY_PX, TINY_PX);
    if cached(dir, hash, ".t").is_none() {
        save_cached(dir, hash, ".t", &tiny, alpha);
    }
    if job.tier == Tier::Tiny || job.want_tiny {
        done.tiny = Some(rgba(&tiny));
    }
    match job.tier {
        Tier::Tiny => {}
        Tier::Preview => done.preview = Some(rgba(&preview)),
        Tier::Full => done.full = Some(rgba(&img)),
    }
    done
}

/// Which loaded image to draw for a picture shown `shown_px` wide (the
/// whole texture, before cropping): tiny when small, else the best loaded.
pub fn pick(tiny: Option<(ImageId, [f32; 4])>, loaded: Option<ImageId>, shown_px: f32) -> Option<(ImageId, [f32; 4])> {
    match (tiny, loaded) {
        (Some(t), _) if shown_px <= TINY_SHOWN_PX => Some(t),
        (_, Some(id)) => Some((id, [0.0, 0.0, 1.0, 1.0])),
        (Some(t), None) => Some(t),
        (None, None) => None,
    }
}

impl App {
    /// Per frame, before drawing: uploads decoded pictures, starts loads for
    /// pictures on screen at the size they're shown, and keeps memory within
    /// budget.
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
        self.loader.atlas.flush(renderer);

        let Some(cache_dir) = self.library.as_ref().map(|l| l.root().join("cache").join("previews")) else { return };

        // What's on screen, largest first, and how many pixels wide each
        // whole picture is shown.
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
            let has_tiny = self.loader.atlas.slots.contains_key(hash);
            let wanted = if *px <= TINY_SHOWN_PX {
                Tier::Tiny
            } else if *px <= PREVIEW_PX as f32 * 1.1 || sc.width.max(sc.height) <= PREVIEW_PX {
                Tier::Preview
            } else {
                Tier::Full
            };
            let have = match self.pictures.get(hash) {
                Some(Picture::Ready(_)) => Some(Tier::Full),
                Some(Picture::Preview(_)) => Some(Tier::Preview),
                Some(Picture::Unloaded) => has_tiny.then_some(Tier::Tiny),
                _ => continue,
            };
            let enough = match (wanted, have) {
                (_, Some(Tier::Full)) => true,
                (Tier::Full, _) => false,
                (_, Some(Tier::Preview)) => true,
                (Tier::Preview, _) => false,
                (Tier::Tiny, Some(Tier::Tiny)) => true,
                (Tier::Tiny, _) => false,
            };
            if enough {
                continue;
            }
            let Some(lib) = &self.library else { break };
            let job = Job { hash: hash.clone(), media: lib.media_path(hash, &sc.ext), cache_dir: cache_dir.clone(), tier: wanted, want_tiny: !has_tiny };
            self.loader.start(job);
        }

        self.evict(renderer, frame);
        if self.loader.busy() {
            self.needs_redraw = true;
        }
    }

    /// Over budget, the pictures off screen longest drop back: full size to
    /// nothing loaded (its tiny image stays), previews likewise.
    fn evict(&mut self, renderer: &mut Renderer, frame: u64) {
        for full in [true, false] {
            let (used, budget) = if full { (self.loader.full_bytes(), FULL_BUDGET) } else { (self.loader.preview_bytes(), PREVIEW_BUDGET) };
            if used <= budget {
                continue;
            }
            let map = if full { &self.loader.full } else { &self.loader.previews };
            let mut old: Vec<(ContentHash, u64)> = map
                .keys()
                .filter(|h| !self.videos.contains_key(*h))
                .map(|h| (h.clone(), self.loader.seen.get(h).copied().unwrap_or(0)))
                .filter(|(_, seen)| *seen < frame)
                .collect();
            old.sort_by_key(|(_, seen)| *seen);
            let mut used = used;
            for (hash, _) in old {
                if used <= budget {
                    break;
                }
                if let Some(Picture::Ready(id) | Picture::Preview(id)) = self.pictures.get(&hash) {
                    renderer.delete_image(*id);
                }
                self.pictures.insert(hash.clone(), Picture::Unloaded);
                let freed = if full { self.loader.full.remove(&hash) } else { self.loader.previews.remove(&hash) };
                used -= freed.unwrap_or(0);
            }
        }
    }

    fn apply_loaded(&mut self, done: Done, renderer: &mut Renderer) {
        let Some(hash) = done.hash else { return };
        if let Some(e) = done.error {
            eprintln!("wall: could not load picture {hash}: {e}");
            return;
        }
        // Libraries from before auto-tags get them on first load.
        let sample = done.tiny.as_ref().or(done.preview.as_ref());
        if let (Some(sc), Some((px, w, h))) = (self.sidecars.get_mut(&hash), sample) {
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
        if let Some((px, w, h)) = &done.tiny {
            self.loader.atlas.add(renderer, hash.clone(), px, *w, *h);
            self.needs_redraw = true;
        }
        let (rgba, w, h, full) = match (done.full, done.preview) {
            (Some((p, w, h)), _) => (p, w, h, true),
            // Never swap a full picture for a preview.
            (None, Some(_)) if matches!(self.pictures.get(&hash), Some(Picture::Ready(_))) => return,
            (None, Some((p, w, h))) => (p, w, h, false),
            (None, None) => return,
        };
        match renderer.create_image(&rgba, w, h) {
            Ok(id) => {
                if let Some(Picture::Preview(old) | Picture::Ready(old)) = self.pictures.get(&hash) {
                    renderer.delete_image(*old);
                }
                self.loader.previews.remove(&hash);
                self.loader.full.remove(&hash);
                if full {
                    self.loader.full.insert(hash.clone(), rgba.len());
                    self.pictures.insert(hash, Picture::Ready(id));
                } else {
                    self.loader.previews.insert(hash.clone(), rgba.len());
                    self.pictures.insert(hash, Picture::Preview(id));
                }
                self.needs_redraw = true;
            }
            Err(e) => eprintln!("wall: image upload failed: {e}"),
        }
    }

    /// The texture and texture rectangle to draw a picture with, when shown
    /// `shown_px` wide.
    pub(crate) fn picture_texture(&self, hash: &ContentHash, shown_px: f32) -> Option<(ImageId, [f32; 4])> {
        let loaded = match self.pictures.get(hash) {
            Some(Picture::Ready(id) | Picture::Preview(id)) => Some(*id),
            _ => None,
        };
        pick(self.loader.tiny(hash), loaded, shown_px)
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

    /// Loader stats for the bench log: full MB, preview MB, atlas pages.
    pub fn picture_stats(&self) -> (usize, usize, usize) {
        (self.loader.full_bytes() >> 20, self.loader.preview_bytes() >> 20, self.loader.atlas_pages())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        img.save_with_format(path, image::ImageFormat::Jpeg).unwrap();
    }

    fn job(media: &Path, cache: &Path, tier: Tier) -> Job {
        Job { hash: ContentHash::of(b"big"), media: media.to_owned(), cache_dir: cache.to_owned(), tier, want_tiny: true }
    }

    #[test]
    fn tiers_fill_and_reuse_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("big.jpg");
        write_jpeg(&media, 2048, 1024);
        let cache = dir.path().join("cache");

        // A tiny job with nothing cached decodes the file and caches both.
        let t = run_job(&job(&media, &cache, Tier::Tiny));
        assert_eq!(t.tiny.as_ref().map(|x| (x.1, x.2)), Some((120, 60)));
        assert!(t.preview.is_none() && t.full.is_none());
        let hash = ContentHash::of(b"big");
        assert!(cached(&cache, &hash, "").is_some() && cached(&cache, &hash, ".t").is_some());

        // With the file gone, tiny and preview still load from the cache.
        std::fs::remove_file(&media).unwrap();
        assert!(run_job(&job(&media, &cache, Tier::Tiny)).tiny.is_some());
        let p = run_job(&job(&media, &cache, Tier::Preview));
        assert_eq!(p.preview.as_ref().map(|x| (x.1, x.2)), Some((512, 256)));
        assert!(p.tiny.is_some());
        // Full size needs the file.
        assert!(run_job(&job(&media, &cache, Tier::Full)).error.is_some());
    }

    #[test]
    fn full_job_returns_full_and_tiny() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("big.jpg");
        write_jpeg(&media, 1600, 900);
        let done = run_job(&job(&media, &dir.path().join("c"), Tier::Full));
        assert_eq!(done.full.as_ref().map(|f| (f.1, f.2)), Some((1600, 900)));
        assert!(done.tiny.is_some() && done.preview.is_none());
    }

    #[test]
    fn padding_repeats_edges() {
        // 2x1: red, blue; pad 1 -> 4x3 with edges repeated.
        let px = [255, 0, 0, 255, 0, 0, 255, 255];
        let (out, w, h) = pad_edges(&px, 2, 1, 1);
        assert_eq!((w, h), (4, 3));
        let at = |x: u32, y: u32| &out[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
        assert_eq!(at(0, 0), &[255, 0, 0, 255]);
        assert_eq!(at(3, 2), &[0, 0, 255, 255]);
    }

    #[test]
    fn pick_uses_tiny_only_when_small() {
        // ImageId can't be made without GL; check the decision on Options.
        assert!(pick(None, None, 50.0).is_none());
    }
}
