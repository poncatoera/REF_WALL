use std::fs;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;

use crate::error::IoContext;
use crate::{Board, ContentHash, Error, ItemSidecar, Manifest, Result, FORMAT_NAME, FORMAT_VERSION};

/// An open `.wall` folder.
#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
    manifest: Manifest,
}

/// Result of adding a media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportOutcome {
    Added(ContentHash),
    /// The same bytes were already in the library; nothing was written.
    Duplicate(ContentHash),
}

impl ImportOutcome {
    pub fn hash(&self) -> &ContentHash {
        match self {
            Self::Added(h) | Self::Duplicate(h) => h,
        }
    }
}

const SUBDIRS: [&str; 5] = ["media", "items", "boards", "cache", "trash"];

impl Library {
    /// Creates a new, empty library. Fails if `root` already holds one.
    pub fn create(root: impl AsRef<Path>, name: &str, now_ms: u64) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join("wall.json");
        if manifest_path.exists() {
            return Err(Error::AlreadyExists(manifest_path));
        }
        for dir in SUBDIRS {
            let p = root.join(dir);
            fs::create_dir_all(&p).at(&p)?;
        }
        let manifest = Manifest {
            format: FORMAT_NAME.into(),
            version: FORMAT_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            created_ms: now_ms,
            extra: Default::default(),
        };
        write_json(&manifest_path, &manifest)?;
        Ok(Self { root, manifest })
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join("wall.json");
        if !manifest_path.is_file() {
            return Err(Error::NotALibrary(root));
        }
        let manifest: Manifest = read_json(&manifest_path)?;
        if manifest.format != FORMAT_NAME {
            return Err(Error::NotALibrary(root));
        }
        if manifest.version > FORMAT_VERSION {
            return Err(Error::TooNew { path: manifest_path, found: manifest.version, supported: FORMAT_VERSION });
        }
        // Folders other apps may have left out.
        for dir in SUBDIRS {
            let p = root.join(dir);
            fs::create_dir_all(&p).at(&p)?;
        }
        Ok(Self { root, manifest })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    // ---- media -------------------------------------------------------------

    pub fn media_path(&self, hash: &ContentHash, ext: &str) -> PathBuf {
        self.root.join("media").join(hash.shard()).join(format!("{hash}.{ext}"))
    }

    pub fn has_media(&self, hash: &ContentHash) -> bool {
        self.sidecar_path(hash).is_file()
    }

    /// Stores already-converted media bytes under their hash. The caller
    /// (`wall-media`) is responsible for the 2048 px / 20 MB rules.
    pub fn import_bytes(&self, bytes: &[u8], ext: &str) -> Result<ImportOutcome> {
        let ext = check_ext(ext)?;
        let hash = ContentHash::of(bytes);
        let path = self.media_path(&hash, &ext);
        if path.is_file() {
            return Ok(ImportOutcome::Duplicate(hash));
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).at(dir)?;
        }
        write_atomic(&path, bytes)?;
        Ok(ImportOutcome::Added(hash))
    }

    // ---- sidecars ----------------------------------------------------------

    fn sidecar_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join("items").join(format!("{hash}.json"))
    }

    pub fn write_sidecar(&self, item: &ItemSidecar) -> Result<()> {
        write_json(&self.sidecar_path(&item.hash), item)
    }

    pub fn read_sidecar(&self, hash: &ContentHash) -> Result<ItemSidecar> {
        read_json(&self.sidecar_path(hash))
    }

    /// Every sidecar in the library. Files that fail to parse are returned as
    /// errors next to the good ones, so one bad file never hides the rest.
    pub fn sidecars(&self) -> Result<Vec<Result<ItemSidecar>>> {
        json_files(&self.root.join("items")).map(|v| v.iter().map(|p| read_json(p)).collect())
    }

    // ---- boards ------------------------------------------------------------

    fn board_path(&self, id: Uuid) -> PathBuf {
        self.root.join("boards").join(format!("{id}.json"))
    }

    pub fn write_board(&self, board: &Board) -> Result<()> {
        write_json(&self.board_path(board.id), board)
    }

    pub fn read_board(&self, id: Uuid) -> Result<Board> {
        let path = self.board_path(id);
        let board: Board = read_json(&path)?;
        if board.version > FORMAT_VERSION {
            return Err(Error::TooNew { path, found: board.version, supported: FORMAT_VERSION });
        }
        Ok(board)
    }

    pub fn board_ids(&self) -> Result<Vec<Uuid>> {
        Ok(json_files(&self.root.join("boards"))?
            .iter()
            .filter_map(|p| p.file_stem()?.to_str()?.parse().ok())
            .collect())
    }

    /// Moves a board file to `trash/`. Nothing in a library is hard-deleted.
    pub fn trash_board(&self, id: Uuid, now_ms: u64) -> Result<PathBuf> {
        let from = self.board_path(id);
        let to = self.root.join("trash").join(format!("{now_ms}-board-{id}.json"));
        fs::rename(&from, &to).at(&from)?;
        Ok(to)
    }
}

fn check_ext(ext: &str) -> Result<String> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let ok = !ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    if ok { Ok(ext) } else { Err(Error::BadExtension(ext)) }
}

fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).at(dir)? {
        let path = entry.at(dir)?.path();
        if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).at(path)?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Json { path: path.to_path_buf(), source })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| Error::Json { path: path.to_path_buf(), source })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Writes to a temp file next to `path`, then renames, so a crash or a sync
/// client never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    fs::write(&tmp, bytes).at(&tmp)?;
    fs::rename(&tmp, path).at(path)
}
