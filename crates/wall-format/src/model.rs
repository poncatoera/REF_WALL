use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::ContentHash;

/// Unknown fields, kept so files written by other apps round-trip intact.
pub type Extra = Map<String, Value>;

/// `wall.json` at the root of a library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub id: Uuid,
    pub name: String,
    pub created_ms: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
}

/// Size of the file before Wall shrank it on import (images over 2048 px wide,
/// videos over 20 MB).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginalInfo {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// Set when the original file was kept, e.g. `media/originals/<hash>.mov`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_as: Option<ContentHash>,
}

/// A tag suggested by a machine. Kept apart from `tags`, which are always the
/// user's own, so re-running auto-tagging never overwrites what someone typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoTag {
    pub tag: String,
    /// Who suggested it, e.g. `"local:color"` or `"claude"`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// `items/<hash>.json`: what Wall knows about one media file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemSidecar {
    pub hash: ContentHash,
    pub kind: MediaKind,
    /// File extension without the dot, lowercase: `jpg`, `png`, `webp`, `mp4`.
    pub ext: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
    pub added_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub auto_tags: Vec<AutoTag>,
    /// Dominant colours as `#rrggbb`, most common first.
    #[serde(default)]
    pub colors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<OriginalInfo>,
    /// Video in and out points: playback loops between them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ClipRange>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A part of a video, in milliseconds from the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipRange {
    pub in_ms: u64,
    pub out_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// How a picture or video is shown. `crop` is in the source's own
/// coordinates; the look applies after it. Mirroring happens on screen,
/// after turning, so "mirror" always flips left and right as you see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Look {
    /// Quarter turns clockwise, 0–3.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turns: u8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mirror: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip_v: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub grayscale: bool,
}

impl Look {
    pub fn is_plain(&self) -> bool {
        *self == Look::default()
    }

    /// True when the picture is on its side (a quarter or three-quarter turn).
    pub fn sideways(&self) -> bool {
        self.turns % 2 == 1
    }
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Where an item sits on the board, in board units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub rotation: f64,
    /// Stacking order; higher draws on top.
    #[serde(default)]
    pub z: f64,
    /// Size of a card's text and controls: 2 draws them twice as large.
    /// Cards made while zoomed far out use it so they stay readable.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub text_scale: f64,
}

impl Default for Frame {
    fn default() -> Self {
        Frame { x: 0.0, y: 0.0, w: 0.0, h: 0.0, rotation: 0.0, z: 0.0, text_scale: 1.0 }
    }
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoEntry {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "to", rename_all = "lowercase")]
pub enum ArrowEnd {
    Item { id: Uuid },
    Point { x: f64, y: f64 },
}

/// What an item is. Each kind has its own field names on purpose: the old Wall
/// had a bug where two kinds shared a `strokes` field and drew each other's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ItemKind {
    Media {
        hash: ContentHash,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crop: Option<Rect>,
        /// Turned, mirrored or black-and-white. Left out when plain.
        #[serde(default, skip_serializing_if = "Look::is_plain")]
        look: Look,
    },
    Note {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    Label {
        text: String,
    },
    Todo {
        title: String,
        entries: Vec<TodoEntry>,
    },
    Group {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
    },
    Arrow {
        from: ArrowEnd,
        to: ArrowEnd,
    },
    /// A board inside this board.
    Board {
        board_id: Uuid,
    },
}

/// One thing on a board. `rev` and `updated_by` let two people (or the desktop
/// app and the web) edit the same board and merge item by item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardItem {
    pub id: Uuid,
    pub frame: Frame,
    #[serde(flatten)]
    pub kind: ItemKind,
    /// Lamport counter: bumped on every change to this item.
    pub rev: u64,
    pub updated_ms: u64,
    /// Stable id of the person or device that made the last change.
    #[serde(default)]
    pub updated_by: String,
    /// Deleted items stay as tombstones so a deletion syncs like any edit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

impl BoardItem {
    /// True when `self` should replace `other` in a merge. Higher `rev` wins;
    /// on a tie the larger `updated_by` wins, so every replica picks the same one.
    pub fn supersedes(&self, other: &BoardItem) -> bool {
        (self.rev, &self.updated_by) > (other.rev, &other.updated_by)
    }
}

/// `boards/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub version: u32,
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Uuid>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub items: BTreeMap<Uuid, BoardItem>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Board {
    pub fn new(name: impl Into<String>, now_ms: u64) -> Self {
        Self {
            version: crate::FORMAT_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            parent: None,
            created_ms: now_ms,
            updated_ms: now_ms,
            items: BTreeMap::new(),
            extra: Extra::new(),
        }
    }

    /// Items that are not deleted.
    pub fn live_items(&self) -> impl Iterator<Item = &BoardItem> {
        self.items.values().filter(|i| !i.deleted)
    }

    /// Merges one incoming item. Returns true if the board changed.
    pub fn merge_item(&mut self, incoming: BoardItem) -> bool {
        match self.items.get(&incoming.id) {
            Some(current) if !incoming.supersedes(current) => false,
            _ => {
                self.updated_ms = self.updated_ms.max(incoming.updated_ms);
                self.items.insert(incoming.id, incoming);
                true
            }
        }
    }

    /// Merges every item of another copy of the same board.
    /// The result is the same whichever order replicas are merged in.
    pub fn merge(&mut self, other: &Board) -> usize {
        other.items.values().filter(|i| self.merge_item((*i).clone())).count()
    }
}
