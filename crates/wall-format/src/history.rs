//! Earlier versions of boards, and boards in the trash.
//!
//! ```text
//! history/<board-id>/<ms>.json.gz   the whole board as it was at <ms>
//! trash/<ms>-board-<board-id>.json  a deleted board (see Library::trash_board)
//! ```
//!
//! A version is the board file as it was just before Wall overwrote it, kept
//! at most every [`VERSION_EVERY_MS`] per board, so the newest version is
//! where the board stood before the latest stretch of editing. Old versions
//! are thinned by [`thin`]: everything from the last day, one an hour for a
//! week, one a day for three months, one a week after that. Boards are small
//! (pictures and videos are only named by hash), and gzip makes them smaller.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use uuid::Uuid;

use crate::error::IoContext;
use crate::{Board, Error, Library, Result, FORMAT_VERSION};

/// How often a board being edited gets a new version.
pub const VERSION_EVERY_MS: u64 = 10 * 60 * 1000;

const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

/// A board in the trash.
#[derive(Debug, Clone, PartialEq)]
pub struct TrashedBoard {
    /// When it was deleted. Boards deleted together (a board and the boards
    /// inside it) share it.
    pub deleted_ms: u64,
    pub board: Board,
}

/// Which of these version times (ms) to remove, so that what's left is:
/// every version from the last day, the newest in each hour for a week, in
/// each day for 90 days, and in each week before that.
pub fn thin(times: &[u64], now_ms: u64) -> Vec<u64> {
    let mut sorted: Vec<u64> = times.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in sorted {
        let age = now_ms.saturating_sub(t);
        let bucket = if age < DAY {
            continue;
        } else if age < 7 * DAY {
            (1, t / HOUR)
        } else if age < 90 * DAY {
            (2, t / DAY)
        } else {
            (3, t / (7 * DAY))
        };
        // Newest first: the first one seen in a bucket stays.
        if !seen.insert(bucket) {
            out.push(t);
        }
    }
    out
}

/// A board from gzipped JSON (a version file, or one the server sent).
/// `path` only names it in errors.
pub fn board_from_gz(gz: &[u8], path: &std::path::Path) -> Result<Board> {
    let mut json = Vec::new();
    // A version is a board's JSON; no board is anywhere near this big.
    flate2::read::GzDecoder::new(gz).take(200_000_000).read_to_end(&mut json).at(path)?;
    let b: Board = serde_json::from_slice(&json).map_err(|source| Error::Json { path: path.to_path_buf(), source })?;
    if b.version > FORMAT_VERSION {
        return Err(Error::TooNew { path: path.to_path_buf(), found: b.version, supported: FORMAT_VERSION });
    }
    Ok(b)
}

impl Library {
    fn history_dir(&self, board: Uuid) -> PathBuf {
        self.root().join("history").join(board.to_string())
    }

    fn version_path(&self, board: Uuid, at_ms: u64) -> PathBuf {
        self.history_dir(board).join(format!("{at_ms}.json.gz"))
    }

    /// When each kept version of a board was made, newest first.
    pub fn versions(&self, board: Uuid) -> Result<Vec<u64>> {
        let dir = self.history_dir(board);
        let Ok(entries) = fs::read_dir(&dir) else { return Ok(Vec::new()) };
        let mut out: Vec<u64> = entries
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json.gz")?.parse().ok())
            .collect();
        out.sort_unstable_by(|a, b| b.cmp(a));
        Ok(out)
    }

    /// A board as it was at one of [`Self::versions`].
    pub fn read_version(&self, board: Uuid, at_ms: u64) -> Result<Board> {
        let path = self.version_path(board, at_ms);
        let gz = fs::read(&path).at(&path)?;
        board_from_gz(&gz, &path)
    }

    /// Keeps `board` as a version made at `at_ms` (if there isn't one at that
    /// time already), then thins the board's versions. True if it wrote one.
    pub fn keep_version(&self, board: &Board, at_ms: u64, now_ms: u64) -> Result<bool> {
        // Another version at the same time: the same one, or a different
        // board state within the same millisecond (put just after it).
        let mut at_ms = at_ms;
        let mut path = self.version_path(board.id, at_ms);
        while path.is_file() {
            if self.read_version(board.id, at_ms).is_ok_and(|v| v == *board) {
                return Ok(false);
            }
            at_ms += 1;
            path = self.version_path(board.id, at_ms);
        }
        let dir = self.history_dir(board.id);
        fs::create_dir_all(&dir).at(&dir)?;
        let json = serde_json::to_vec(board).map_err(|source| Error::Json { path: path.clone(), source })?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&json).at(&path)?;
        let bytes = gz.finish().at(&path)?;
        let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
        fs::write(&tmp, &bytes).at(&tmp)?;
        fs::rename(&tmp, &path).at(&path)?;
        for t in thin(&self.versions(board.id)?, now_ms) {
            let p = self.version_path(board.id, t);
            fs::remove_file(&p).at(&p)?;
        }
        Ok(true)
    }

    /// Keeps the board file as it is on disk now as a version, dated when it
    /// last changed. Nothing happens if the board has no file yet.
    pub fn keep_saved_version(&self, board: Uuid, now_ms: u64) -> Result<bool> {
        let Ok(old) = self.read_board(board) else { return Ok(false) };
        // A board that never had anything on it isn't worth going back to.
        if old.items.is_empty() {
            return Ok(false);
        }
        let at = if old.updated_ms > 0 { old.updated_ms } else { now_ms };
        self.keep_version(&old, at, now_ms)
    }

    // ---- trashed boards ----------------------------------------------------

    /// Trash files of deleted boards as (deleted_ms, id, path), newest first.
    fn board_trash_files(&self) -> Result<Vec<(u64, Uuid, PathBuf)>> {
        let trash = self.root().join("trash");
        let mut out = Vec::new();
        for entry in fs::read_dir(&trash).at(&trash)? {
            let path = entry.at(&trash)?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { continue };
            let Some((ms, id)) = name.split_once("-board-") else { continue };
            if let (Ok(ms), Ok(id)) = (ms.parse::<u64>(), id.parse::<Uuid>()) {
                out.push((ms, id, path));
            }
        }
        out.sort_by_key(|x| std::cmp::Reverse(x.0));
        Ok(out)
    }

    /// Boards in the trash, newest first. A board trashed twice is listed
    /// once (its latest), and not at all if it's back in the library.
    pub fn trashed_boards(&self) -> Result<Vec<TrashedBoard>> {
        let live: std::collections::HashSet<Uuid> = self.board_ids()?.into_iter().collect();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (ms, id, path) in self.board_trash_files()? {
            if live.contains(&id) || !seen.insert(id) {
                continue;
            }
            if let Ok(board) = crate::library::read_json::<Board>(&path) {
                out.push(TrashedBoard { deleted_ms: ms, board });
            }
        }
        Ok(out)
    }

    /// Puts the most recently trashed copy of a board back and returns it.
    pub fn restore_board(&self, id: Uuid) -> Result<Board> {
        let Some((_, _, path)) = self.board_trash_files()?.into_iter().find(|(_, b, _)| *b == id) else {
            return Err(Error::NotInTrash(id.to_string()));
        };
        let board: Board = crate::library::read_json(&path)?;
        if self.read_board(id).is_ok() {
            return Err(Error::AlreadyExists(self.root().join("boards").join(format!("{id}.json"))));
        }
        self.write_board(&board)?;
        fs::remove_file(&path).at(&path)?;
        Ok(board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinning_keeps_a_day_then_hours_days_and_weeks() {
        let now = 400 * DAY;
        // Every 10 minutes for the last 200 days.
        let times: Vec<u64> = (0..200 * 144).map(|k| now - k * 600_000).collect();
        let gone: std::collections::HashSet<u64> = thin(&times, now).into_iter().collect();
        let kept: Vec<u64> = times.iter().copied().filter(|t| !gone.contains(t)).collect();
        let within = |lo: u64, hi: u64| kept.iter().filter(|t| now - **t >= lo && now - **t < hi).count();
        assert_eq!(within(0, DAY), 144, "the last day is untouched");
        assert!((143..=145).contains(&within(DAY, 7 * DAY)), "hourly for a week: {}", within(DAY, 7 * DAY));
        assert!((82..=84).contains(&within(7 * DAY, 90 * DAY)), "daily to 90 days: {}", within(7 * DAY, 90 * DAY));
        assert!((15..=17).contains(&within(90 * DAY, 200 * DAY)), "weekly after: {}", within(90 * DAY, 200 * DAY));
        // Thinning again changes nothing.
        assert!(thin(&kept, now).is_empty());
    }
}
