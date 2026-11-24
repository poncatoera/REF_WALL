//! The `.wall` library format.
//!
//! A library is a plain folder that any program can read without Wall:
//!
//! ```text
//! MyRefs.wall/
//!   wall.json              manifest: format version, library id, name
//!   media/ab/ab12….mp4     media files named by BLAKE3 hash, never edited
//!   items/ab12….json       one sidecar per media file: size, tags, colours, source
//!   boards/<uuid>.json     one file per board
//!   cache/                 previews and thumbnails, safe to delete
//!   trash/                 nothing is hard-deleted
//!   history/<uuid>/<ms>.json.gz  earlier versions of each board (see `history`)
//! ```
//!
//! Every JSON object keeps fields it does not know about (`extra`), so a file
//! written by another app round-trips through Wall without losing data.

pub mod backup;
mod error;
mod hash;
pub mod history;
mod library;
mod model;

pub use backup::BackupReport;
pub use error::{Error, Result};
pub use hash::ContentHash;
pub use history::{TrashedBoard, VERSION_EVERY_MS};
pub use library::{ImportOutcome, Library};
pub use model::*;

/// Version of the on-disk format written by this crate.
pub const FORMAT_VERSION: u32 = 1;
/// Value of the `format` field in `wall.json`.
pub const FORMAT_NAME: &str = "wall";
