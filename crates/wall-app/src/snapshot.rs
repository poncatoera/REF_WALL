//! Saving without a library folder (the web version): every board and what
//! Wall knows about each picture and video, as one JSON document, plus the
//! media files themselves, which the shell stores by content hash.
//!
//! A shell calls [`App::edit_stamp`] now and then; when it changes, it saves
//! [`App::snapshot`] and the files from [`App::take_new_media`]. At start it
//! hands a saved snapshot and the stored files back to [`App::restore`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use wall_format::{Board, ContentHash, ItemKind, ItemSidecar, MediaKind};

use crate::{App, AppError, Camera, Document, Gesture, Picture};

const VERSION: u64 = 1;

/// A media file imported while no library is open, for the shell to keep.
#[derive(Debug, Clone)]
pub struct NewMedia {
    pub hash: ContentHash,
    pub ext: String,
    pub bytes: Vec<u8>,
}

impl App {
    /// Changes whenever something worth saving changes: any board's items,
    /// board names, picture titles and tags. Cheap enough to call each second.
    pub fn edit_stamp(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.doc.board.id.hash(&mut h);
        self.doc.version.hash(&mut h);
        [self.camera.center[0], self.camera.center[1], self.camera.zoom].map(f32::to_bits).hash(&mut h);
        let mut parked: Vec<_> = self.parked.iter().map(|(id, (d, _))| (*id, d.version)).collect();
        parked.sort();
        parked.hash(&mut h);
        let mut names: Vec<_> = self.board_names.iter().collect();
        names.sort();
        names.hash(&mut h);
        let mut sidecars: Vec<_> = self.sidecars.values().map(|s| serde_json::to_string(s).unwrap_or_default()).collect();
        sidecars.sort();
        sidecars.hash(&mut h);
        h.finish()
    }

    /// True while a drag or typing is in progress: save after it ends, so
    /// a half-finished edit is never what comes back.
    pub fn busy_editing(&self) -> bool {
        !matches!(self.gesture, Gesture::Idle) || self.editing.is_some() || self.history_previewing()
    }

    /// Every board (the open one and the others visited or made) and every
    /// sidecar, as JSON.
    pub fn snapshot(&self) -> String {
        let mut boards = vec![&self.doc.board];
        boards.extend(self.parked.values().map(|(d, _)| &d.board));
        boards.sort_by_key(|b| (b.parent.is_some(), b.created_ms));
        let mut sidecars: Vec<&ItemSidecar> = self.sidecars.values().collect();
        sidecars.sort_by(|a, b| a.hash.as_str().cmp(b.hash.as_str()));
        serde_json::json!({
            "version": VERSION,
            "open": self.doc.board.id,
            "camera": [self.camera.center[0], self.camera.center[1], self.camera.zoom],
            "boards": boards,
            "sidecars": sidecars,
        })
        .to_string()
    }

    /// Media files imported since the last call (only kept when no library
    /// folder is open; a library stores its own).
    pub fn take_new_media(&mut self) -> Vec<NewMedia> {
        std::mem::take(&mut self.new_media)
    }

    /// The media files a snapshot needs: hash, extension, video or not.
    pub fn snapshot_media(json: &str) -> Vec<(ContentHash, String, bool)> {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
        let sidecars: Vec<ItemSidecar> = v.get("sidecars").cloned().and_then(|s| serde_json::from_value(s).ok()).unwrap_or_default();
        sidecars.into_iter().map(|s| (s.hash, s.ext, s.kind == MediaKind::Video)).collect()
    }

    /// Replaces the boards with a saved snapshot and opens the top board.
    /// `media` holds the stored files by hash; pictures already known (the
    /// demo's) are kept. Returns the videos the shell should set players up
    /// for: hash, extension, width, height.
    pub fn restore(&mut self, json: &str, media: &HashMap<ContentHash, Vec<u8>>) -> Result<Vec<(ContentHash, String, u32, u32)>, AppError> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| AppError::Other(format!("saved boards: {e}")))?;
        if v.get("version").and_then(|x| x.as_u64()) != Some(VERSION) {
            return Err(AppError::Other("saved boards are from another version".into()));
        }
        let boards: Vec<Board> = serde_json::from_value(v["boards"].clone()).map_err(|e| AppError::Other(format!("saved boards: {e}")))?;
        let sidecars: Vec<ItemSidecar> = serde_json::from_value(v["sidecars"].clone()).unwrap_or_default();
        let Some(top) = boards.iter().position(|b| b.parent.is_none()).or(if boards.is_empty() { None } else { Some(0) }) else {
            return Err(AppError::Other("saved boards are empty".into()));
        };
        let mut boards = boards;
        let top = boards.remove(top);
        let device = self.doc.device.clone();
        self.end_edit();
        self.board_names = std::iter::once(&top).chain(&boards).map(|b| (b.id, b.name.clone())).collect();
        self.parked = boards.into_iter().map(|b| (b.id, (Document::new(b, device.clone()), Camera { center: [0.0, 0.0], zoom: 1.0 }))).collect();
        self.doc = Document::new(top, device);
        self.nav.clear();
        self.selection.clear();
        self.gesture = Gesture::Idle;
        self.scene.invalidate();

        let mut videos = Vec::new();
        for sc in sidecars {
            let hash = sc.hash.clone();
            if sc.kind == MediaKind::Video {
                if media.contains_key(&hash) {
                    self.videos.insert(hash.clone(), (sc.ext.clone(), sc.width, sc.height));
                    videos.push((hash.clone(), sc.ext.clone(), sc.width, sc.height));
                }
            } else if !self.pictures.contains_key(&hash) {
                if let Some(rgba) = media.get(&hash).and_then(|b| image::load_from_memory(b).ok()).map(|i| i.to_rgba8()) {
                    let (width, height) = rgba.dimensions();
                    self.pictures.insert(hash.clone(), Picture::Pending { rgba: rgba.into_raw(), width, height });
                }
            }
            self.sidecars.insert(hash, sc);
        }
        // The view as it was left, else everything on the board.
        let camera: Option<[f32; 3]> = v.get("camera").and_then(|c| serde_json::from_value(c.clone()).ok());
        match camera {
            Some([x, y, zoom]) if zoom > 0.0 => {
                self.camera = Camera { center: [x, y], zoom };
                self.fit_on_resize = false;
            }
            _ if self.viewport[0] > 1.0 => self.fit_view(),
            _ => self.fit_on_resize = true,
        }
        self.needs_redraw = true;
        Ok(videos)
    }

    /// Hashes of media on any board that has no pixels or player (missing
    /// from what was saved).
    pub fn missing_media(&self) -> Vec<ContentHash> {
        let mut out = Vec::new();
        for b in std::iter::once(&self.doc.board).chain(self.parked.values().map(|(d, _)| &d.board)) {
            for i in b.live_items() {
                if let ItemKind::Media { hash, .. } = &i.kind {
                    if !self.pictures.contains_key(hash) && !self.videos.contains_key(hash) && !out.contains(hash) {
                        out.push(hash.clone());
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Board;

    fn png() -> Vec<u8> {
        let img = image::RgbaImage::from_fn(8, 4, |x, _| image::Rgba([x as u8 * 30, 10, 200, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn boards_and_pictures_come_back_from_a_snapshot() {
        let mut a = App::new(Board::new("Mine", 0), "web");
        a.resize([800.0, 600.0]);
        let stamp = a.edit_stamp();
        a.new_note([10.0, 10.0], "hello".into());
        a.end_edit();
        a.import_image_bytes(&png(), Some("dots.png"), 5).unwrap();
        assert_ne!(a.edit_stamp(), stamp);
        let child = a.new_board();
        let _ = child;
        let media = a.take_new_media();
        assert_eq!(media.len(), 1);
        let json = a.snapshot();
        assert_eq!(App::snapshot_media(&json).len(), 1);

        let mut b = App::new(Board::new("Demo", 0), "web");
        b.resize([800.0, 600.0]);
        let files: HashMap<_, _> = media.into_iter().map(|m| (m.hash, m.bytes)).collect();
        let videos = b.restore(&json, &files).unwrap();
        assert!(videos.is_empty());
        assert_eq!(b.board_name(), "Mine");
        assert_eq!(b.board_names.len(), 2);
        assert_eq!(b.item_count(), a.item_count());
        assert!(b.missing_media().is_empty());
        // Saving again gives the same boards.
        assert_eq!(serde_json::from_str::<serde_json::Value>(&b.snapshot()).unwrap()["boards"], serde_json::from_str::<serde_json::Value>(&json).unwrap()["boards"]);
    }
}
