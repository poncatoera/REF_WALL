//! One Wall at a time per library. Two Walls saving the same library would
//! overwrite each other's boards, which became easier to do once the
//! installer added a Start menu shortcut and "Open in Wall" on `.wall`
//! folders. A Wall holds an OS file lock on `cache/open.lock` inside the
//! library while it has the library open; the OS drops the lock when Wall
//! exits or crashes, so a stale lock can't keep the library shut.
//! (`cache/` is the library's safe-to-delete folder; see docs/wall-format.md.)

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The library this Wall has open and the handle holding its lock.
static HELD: Mutex<Option<(PathBuf, File)>> = Mutex::new(None);

/// Why a library couldn't be locked.
#[derive(Debug)]
pub enum LockError {
    /// Another Wall has it open.
    Busy,
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Busy => f.write_str("this library is already open in another Wall window"),
            LockError::Io(e) => write!(f, "could not lock the library: {e}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Locks `library` for this Wall and lets go of the library it had before.
/// Locking the library it already holds is fine.
pub fn take(library: &Path) -> Result<(), LockError> {
    lock_into(&mut HELD.lock().unwrap_or_else(|e| e.into_inner()), library)
}

fn lock_into(held: &mut Option<(PathBuf, File)>, library: &Path) -> Result<(), LockError> {
    let key = std::fs::canonicalize(library).unwrap_or_else(|_| library.to_path_buf());
    if held.as_ref().is_some_and(|(p, _)| *p == key) {
        return Ok(());
    }
    let dir = library.join("cache");
    std::fs::create_dir_all(&dir).map_err(LockError::Io)?;
    let file = File::options().create(true).truncate(false).write(true).open(dir.join("open.lock")).map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(LockError::Busy),
        Err(std::fs::TryLockError::Error(e)) => return Err(LockError::Io(e)),
    }
    // Dropping the old handle releases the old library.
    *held = Some((key, file));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_wall_cannot_open_the_same_library() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A.wall");
        let b = dir.path().join("B.wall");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // A slot of its own, so tests running at the same time don't mix.
        let mut held = None;
        lock_into(&mut held, &a).unwrap();
        lock_into(&mut held, &a).unwrap(); // the same Wall again: fine
        // Another process: a separate handle on the same lock file.
        let other = File::options().write(true).open(a.join("cache").join("open.lock")).unwrap();
        assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        // Switching to B lets go of A.
        lock_into(&mut held, &b).unwrap();
        other.try_lock().unwrap();
    }
}
