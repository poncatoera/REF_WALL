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

/// A lock on a library that this Wall hasn't switched to yet.
pub struct Pending(Option<(PathBuf, File)>);

/// Locks `library` before Wall reads it, so no other Wall can save it in
/// between. The library Wall had open stays locked until `keep`. Locking
/// the library this Wall already holds is fine.
pub fn take(library: &Path) -> Result<Pending, LockError> {
    let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    lock(held.as_ref(), library).map(Pending)
}

/// Switches this Wall to the library of `pending` and lets go of the old one.
pub fn keep(pending: Pending) {
    if let Some(new) = pending.0 {
        // Dropping the old handle releases the old library.
        *HELD.lock().unwrap_or_else(|e| e.into_inner()) = Some(new);
    }
}

/// Lets go of the open library, as when Wall exits.
#[cfg(test)]
pub fn release() {
    *HELD.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// `None` when `held` is already this library.
fn lock(held: Option<&(PathBuf, File)>, library: &Path) -> Result<Option<(PathBuf, File)>, LockError> {
    let dir = library.join("cache");
    std::fs::create_dir_all(&dir).map_err(LockError::Io)?;
    // After creating it: a new library's path can't be resolved before.
    let key = std::fs::canonicalize(library).unwrap_or_else(|_| library.to_path_buf());
    if held.is_some_and(|(p, _)| *p == key) {
        return Ok(None);
    }
    let file = File::options().create(true).truncate(false).write(true).open(dir.join("open.lock")).map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(Some((key, file))),
        Err(std::fs::TryLockError::WouldBlock) => Err(LockError::Busy),
        Err(std::fs::TryLockError::Error(e)) => Err(LockError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_library_can_be_opened_again() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("New.wall");
        let held = lock(None, &new).unwrap();
        assert!(lock(held.as_ref(), &new).unwrap().is_none());
    }

    #[test]
    fn a_second_wall_cannot_open_the_same_library() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A.wall");
        let b = dir.path().join("B.wall");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // Its own slot, so tests running at the same time don't mix.
        let mut held = lock(None, &a).unwrap();
        assert!(lock(held.as_ref(), &a).unwrap().is_none()); // the same Wall again: fine
        // Another process: a separate handle on the same lock file.
        let other = File::options().write(true).open(a.join("cache").join("open.lock")).unwrap();
        assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        // Switching to B lets go of A.
        held = lock(held.as_ref(), &b).unwrap();
        assert!(held.is_some());
        // Retried: on Linux a child process another test is starting can
        // share the handle for a moment between fork and exec.
        let freed = (0..50).any(|_| other.try_lock().is_ok() || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            false
        });
        assert!(freed);
    }
}
