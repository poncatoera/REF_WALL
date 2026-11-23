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
            board_order: Vec::new(),
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

    /// Saves the order boards are listed in (see [`Manifest::board_order`]).
    pub fn set_board_order(&mut self, order: Vec<Uuid>) -> Result<()> {
        self.manifest.board_order = order;
        write_json(&self.root.join("wall.json"), &self.manifest)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    // ---- media -------------------------------------------------------------

    /// Where a media file lives. `ext` usually comes from a sidecar, which
    /// anyone can edit, so anything but letters and digits is dropped: a
    /// crafted `ext` like `../../x` can never point outside the library.
    pub fn media_path(&self, hash: &ContentHash, ext: &str) -> PathBuf {
        let mut ext: String = ext.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        ext.make_ascii_lowercase();
        if ext.is_empty() {
            ext.push_str("bin");
        }
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

    // ---- media trash -------------------------------------------------------

    /// Moves a picture or video and its sidecar into
    /// `trash/<now_ms>-media-<hash>/`. Boards that use it show a placeholder
    /// until it is restored. Nothing is hard-deleted.
    pub fn trash_media(&self, hash: &ContentHash, now_ms: u64) -> Result<PathBuf> {
        let sc = self.read_sidecar(hash)?;
        let dir = self.root.join("trash").join(format!("{now_ms}-media-{hash}"));
        fs::create_dir_all(&dir).at(&dir)?;
        let media = self.media_path(hash, &sc.ext);
        if media.is_file() {
            fs::rename(&media, dir.join(format!("{hash}.{}", sc.ext))).at(&media)?;
        }
        let side = self.sidecar_path(hash);
        fs::rename(&side, dir.join(format!("{hash}.json"))).at(&side)?;
        Ok(dir)
    }

    /// Trash folders of trashed media, newest first, as (folder, hash).
    fn media_trash_dirs(&self) -> Result<Vec<(PathBuf, String)>> {
        let trash = self.root.join("trash");
        let mut out = Vec::new();
        for entry in fs::read_dir(&trash).at(&trash)? {
            let path = entry.at(&trash)?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if let Some((_, hash)) = name.split_once("-media-").filter(|_| path.is_dir()) {
                out.push((path.clone(), hash.to_owned()));
            }
        }
        // Names start with the trash time in ms: sort newest first.
        out.sort_by_key(|(p, _)| {
            let t: u64 = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('-').next()?.parse().ok()).unwrap_or(0);
            std::cmp::Reverse(t)
        });
        Ok(out)
    }

    /// Sidecars of trashed media, newest first. A hash trashed twice is
    /// listed once.
    pub fn trashed_media(&self) -> Result<Vec<ItemSidecar>> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (dir, hash) in self.media_trash_dirs()? {
            if !seen.insert(hash.clone()) {
                continue;
            }
            if let Ok(sc) = read_json::<ItemSidecar>(&dir.join(format!("{hash}.json"))) {
                out.push(sc);
            }
        }
        Ok(out)
    }

    /// Puts the most recently trashed copy of `hash` back.
    pub fn restore_media(&self, hash: &ContentHash) -> Result<()> {
        let key = hash.to_string();
        let Some((dir, _)) = self.media_trash_dirs()?.into_iter().find(|(_, h)| *h == key) else {
            return Err(Error::NotInTrash(key));
        };
        let sc: ItemSidecar = read_json(&dir.join(format!("{hash}.json")))?;
        let from = dir.join(format!("{hash}.{}", sc.ext));
        if from.is_file() {
            let to = self.media_path(hash, &sc.ext);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).at(parent)?;
            }
            fs::rename(&from, &to).at(&from)?;
        }
        let side = dir.join(format!("{hash}.json"));
        fs::rename(&side, self.sidecar_path(hash)).at(&side)?;
        fs::remove_dir(&dir).at(&dir)?;
        Ok(())
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

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).at(path)?;
    // Windows tools (PowerShell 5, Notepad) often save UTF-8 with a BOM;
    // serde_json refuses it, so skip it rather than fail on another app's file.
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    serde_json::from_slice(json).map_err(|source| Error::Json { path: path.to_path_buf(), source })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = to_json(value).map_err(|source| Error::Json { path: path.to_path_buf(), source })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// JSON with objects indented (readable, diffable) and arrays on one line,
/// so a drawn line's hundreds of numbers don't take a line each.
pub fn to_json<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, Readable::default());
    value.serialize(&mut ser)?;
    Ok(out)
}

#[derive(Default)]
struct Readable {
    depth: usize,
    /// Per open object: whether it has a field yet.
    filled: Vec<bool>,
}

impl Readable {
    fn newline<W: ?Sized + std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(b"\n")?;
        for _ in 0..self.depth {
            w.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl serde_json::ser::Formatter for Readable {
    fn begin_object<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.depth += 1;
        self.filled.push(false);
        w.write_all(b"{")
    }
    fn end_object<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.depth -= 1;
        if self.filled.pop().unwrap_or(false) {
            self.newline(w)?;
        }
        w.write_all(b"}")
    }
    fn begin_object_key<W: ?Sized + std::io::Write>(&mut self, w: &mut W, first: bool) -> std::io::Result<()> {
        if let Some(f) = self.filled.last_mut() {
            *f = true;
        }
        if !first {
            w.write_all(b",")?;
        }
        self.newline(w)
    }
    fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        w.write_all(b": ")
    }
}

/// Writes to a temp file next to `path`, then renames, so a crash or a sync
/// client never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    fs::write(&tmp, bytes).at(&tmp)?;
    fs::rename(&tmp, path).at(path)
}

#[cfg(test)]
mod json_tests {
    #[test]
    fn objects_are_indented_and_arrays_stay_on_one_line() {
        let v = serde_json::json!({"a": 1, "points": [[0.5, 0.25], [1.0, 0.0]], "o": {}, "list": [{"x": 1}]});
        let s = String::from_utf8(super::to_json(&v).unwrap()).unwrap();
        assert_eq!(s, "{\n  \"a\": 1,\n  \"list\": [{\n    \"x\": 1\n  }],\n  \"o\": {},\n  \"points\": [[0.5,0.25],[1.0,0.0]]\n}");
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }
}
