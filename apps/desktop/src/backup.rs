//! Backups of the open library (File > Back up library now / Back up to
//! folder… / Export library as .zip…). The backup folder is kept per
//! library in settings; once there is one, Wall brings it up to date every
//! half hour while open and when it closes. Only what changed is copied
//! (see `wall_format::backup`), so that takes moments. Runs off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use wall_format::{BackupReport, Library};

/// How often a library with a backup folder is backed up while open.
pub const EVERY: Duration = Duration::from_secs(30 * 60);
/// The first automatic backup after opening.
const FIRST_AFTER: Duration = Duration::from_secs(60);

pub enum Done {
    /// To the backup folder; `asked` if from the menu (else automatic).
    BackedUp { to: PathBuf, report: BackupReport, asked: bool },
    Exported { to: PathBuf, report: BackupReport },
    Failed { what: String, asked: bool },
}

pub struct Backups {
    running: Option<Receiver<Done>>,
    next: Instant,
}

impl Default for Backups {
    fn default() -> Self {
        Self { running: None, next: Instant::now() + FIRST_AFTER }
    }
}

/// Today as 2026-11-24 (UTC), for file names.
pub fn today() -> String {
    let days = (crate::now_ms() / 86_400_000) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn key(root: &Path) -> Option<String> {
    Library::open(root).ok().map(|l| format!("backup_folder:{}", l.manifest().id))
}

/// The backup folder chosen for this library, if any.
pub fn folder(root: &Path) -> Option<PathBuf> {
    crate::settings::get_str(&key(root)?).map(PathBuf::from)
}

pub fn set_folder(root: &Path, dest: &Path) {
    if let Some(k) = key(root) {
        crate::settings::set_str(&k, &dest.to_string_lossy());
    }
}

/// What a finished backup or export says to the person.
pub fn describe(d: &Done) -> String {
    let mb = |b: u64| format!("{:.1} MB", b as f64 / 1e6);
    match d {
        Done::BackedUp { to, report, .. } if report.copied == 0 => format!("The backup in {} was already up to date ({} files).", to.display(), report.files),
        Done::BackedUp { to, report, .. } => format!(
            "Backed up to {}: {} new or changed files ({}), {} in all.\n\nWall keeps this backup up to date every 30 minutes while it's open, and when you close it.",
            to.display(),
            report.copied,
            mb(report.bytes),
            report.files
        ),
        Done::Exported { to, report } => format!("Saved the whole library ({} files, {}) as {}.\n\nUnzip it and open the folder inside with File > Open library.", report.files, mb(report.bytes), to.display()),
        Done::Failed { what, .. } => what.clone(),
    }
}

impl Backups {
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Brings `dest` up to date with the library at `root`.
    pub fn back_up(&mut self, root: &Path, dest: &Path, asked: bool) {
        if self.running.is_some() {
            return;
        }
        self.next = Instant::now() + EVERY;
        let (root, dest) = (root.to_owned(), dest.to_owned());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let done = match Library::open(&root).and_then(|l| l.back_up_to(&dest)) {
                Ok(report) => Done::BackedUp { to: dest, report, asked },
                Err(e) => Done::Failed { what: format!("Could not back up to {}: {e}", dest.display()), asked },
            };
            let _ = tx.send(done);
        });
        self.running = Some(rx);
    }

    /// Writes the library at `root` to one .zip file.
    pub fn export(&mut self, root: &Path, to: &Path) {
        if self.running.is_some() {
            return;
        }
        let (root, to) = (root.to_owned(), to.to_owned());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let done = match Library::open(&root).and_then(|l| l.export_zip(&to)) {
                Ok(report) => Done::Exported { to, report },
                Err(e) => Done::Failed { what: format!("Could not export the library: {e}"), asked: true },
            };
            let _ = tx.send(done);
        });
        self.running = Some(rx);
    }

    /// Starts the half-hourly backup when it's time, and hands back one
    /// that finished.
    pub fn tick(&mut self, root: Option<&Path>) -> Option<Done> {
        if let Some(rx) = &self.running {
            return match rx.try_recv() {
                Ok(d) => {
                    self.running = None;
                    Some(d)
                }
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.running = None;
                    None
                }
            };
        }
        if Instant::now() >= self.next {
            self.next = Instant::now() + EVERY;
            if let Some(root) = root {
                if let Some(dest) = folder(root) {
                    self.back_up(root, &dest, false);
                }
            }
        }
        None
    }

    /// When Wall closes: one last backup, waited for (it copies only what
    /// changed since the last one, so it's quick).
    pub fn on_close(&mut self, root: Option<&Path>) {
        if let Some(rx) = self.running.take() {
            let _ = rx.recv_timeout(Duration::from_secs(20));
        }
        let Some(root) = root else { return };
        let Some(dest) = folder(root) else { return };
        match Library::open(root).and_then(|l| l.back_up_to(&dest)) {
            Ok(r) => println!("wall: backed up to {} ({} of {} files copied)", dest.display(), r.copied, r.files),
            Err(e) => eprintln!("wall: could not back up to {}: {e}", dest.display()),
        }
    }
}
