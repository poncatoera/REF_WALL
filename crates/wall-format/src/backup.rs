//! Copies of a library, so nothing in it can be lost:
//!
//! - [`Library::back_up_to`] keeps a plain copy of the library in another
//!   folder (another drive, a synced folder) up to date. Only what changed
//!   is copied: media files never change once written, so after the first
//!   run a backup takes moments. Nothing is ever deleted from the backup,
//!   and the copy is itself a library that Wall opens.
//! - [`Library::export_zip`] writes the whole library as one `.zip` file,
//!   to archive or move it. [`unzip`] puts one back as a folder.
//!
//! Both leave out `cache/` (previews Wall makes again) and both read a
//! file at a time, so libraries of many gigabytes need little memory.
//! The zip is uncompressed (pictures and videos don't compress) and uses
//! ZIP64 where needed, so any unzip tool opens it.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::IoContext;
use crate::{Error, Library, Manifest, Result};

/// Folders of a library that are never copied.
const SKIP: [&str; 1] = ["cache"];

/// What a backup or export did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupReport {
    /// Files in the library (outside `cache/`).
    pub files: u64,
    /// Files copied this time (all of them for an export).
    pub copied: u64,
    /// Bytes copied this time.
    pub bytes: u64,
}

impl Library {
    /// Every file of the library outside `cache/`, as (path inside the
    /// library with `/` separators, full path), sorted.
    pub fn files(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut out = Vec::new();
        walk(self.root(), self.root(), &mut out)?;
        out.sort();
        Ok(out)
    }

    /// Brings the copy of this library in `dest` up to date: copies files
    /// that are new or changed (by size and time), deletes nothing. `dest`
    /// may be empty or missing; it must not hold another library or be
    /// inside this one.
    pub fn back_up_to(&self, dest: &Path) -> Result<BackupReport> {
        check_dest(self, dest)?;
        fs::create_dir_all(dest).at(dest)?;
        let mut r = BackupReport::default();
        for (rel, from) in self.files()? {
            r.files += 1;
            let to = dest.join(&rel);
            let src = fs::metadata(&from).at(&from)?;
            if let Ok(have) = fs::metadata(&to) {
                // FAT drives keep times to 2 seconds.
                let older = match (src.modified(), have.modified()) {
                    (Ok(a), Ok(b)) => a.duration_since(b).is_ok_and(|d| d.as_secs() >= 2),
                    _ => true,
                };
                if have.len() == src.len() && !older {
                    continue;
                }
            }
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir).at(dir)?;
            }
            // Copied beside it, then renamed: a backup stopped halfway never
            // leaves half a file in place of a good one.
            let tmp = to.with_file_name(format!(".{}.part", to.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
            fs::copy(&from, &tmp).at(&from)?;
            if let Ok(t) = src.modified() {
                let _ = File::options().write(true).open(&tmp).and_then(|f| f.set_modified(t));
            }
            fs::rename(&tmp, &to).at(&to)?;
            r.copied += 1;
            r.bytes += src.len();
        }
        Ok(r)
    }

    /// Writes the whole library to one `.zip` file at `dest` (replaced if
    /// there). Entries are inside a folder named after the library's folder,
    /// so unzipping gives a library ready to open.
    pub fn export_zip(&self, dest: &Path) -> Result<BackupReport> {
        let folder = self.root().file_name().and_then(|n| n.to_str()).map(str::to_owned).unwrap_or_else(|| "Library.wall".into());
        let tmp = dest.with_extension("zip.part");
        let mut r = BackupReport::default();
        let result = (|| -> io::Result<()> {
            let mut z = ZipWriter::new(BufWriter::with_capacity(1 << 20, File::create(&tmp)?));
            for (rel, from) in self.files().map_err(io::Error::other)? {
                let mut f = File::open(&from)?;
                let len = f.metadata()?.len();
                let time = f.metadata()?.modified().ok();
                z.add(&format!("{folder}/{rel}"), &mut f, len, time)?;
                r.files += 1;
                r.copied += 1;
                r.bytes += len;
            }
            z.finish()?.flush()
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(Error::Io { path: dest.to_owned(), source: e });
        }
        fs::rename(&tmp, dest).at(dest)?;
        Ok(r)
    }
}

/// A backup folder must be empty, missing, or a backup of the same library.
fn check_dest(lib: &Library, dest: &Path) -> Result<()> {
    let root = full_path(lib.root());
    let full = full_path(dest);
    if full.starts_with(&root) || root.starts_with(&full) {
        return Err(Error::Backup("the backup folder can't be inside the library, or the library inside it".into()));
    }
    let manifest = dest.join("wall.json");
    if manifest.is_file() {
        let theirs: Manifest = crate::library::read_json(&manifest)?;
        if theirs.id != lib.manifest().id {
            return Err(Error::Backup(format!("{} holds another library (\"{}\")", dest.display(), theirs.name)));
        }
    } else if fs::read_dir(dest).is_ok_and(|mut d| d.next().is_some()) {
        return Err(Error::Backup(format!("{} isn't empty; pick an empty folder for the backup", dest.display())));
    }
    Ok(())
}

/// The full path of `p` even when it doesn't exist yet: the nearest folder
/// that does is made full (on Windows that gives the `\\?\C:\` form) and
/// the rest is added, so two paths compare alike either way.
fn full_path(p: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = p.to_path_buf();
    loop {
        if let Ok(full) = fs::canonicalize(&at) {
            return rest.iter().rev().fold(full, |acc: PathBuf, part| acc.join(part));
        }
        match (at.file_name().map(|n| n.to_owned()), at.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                at = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(dir).at(dir)? {
        let entry = entry.at(dir)?;
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        if SKIP.contains(&rel.as_str()) {
            continue;
        }
        let kind = entry.file_type().at(&path)?;
        if kind.is_dir() {
            walk(root, &path, out)?;
        } else if kind.is_file() && !rel.ends_with(".part") {
            out.push((rel, path));
        }
    }
    Ok(())
}

// ---- zip (stored, ZIP64) ------------------------------------------------------

struct Entry {
    name: String,
    crc: u32,
    size: u64,
    offset: u64,
    dos_time: u16,
    dos_date: u16,
}

/// A minimal zip writer: stored entries, sizes known up front, ZIP64 extra
/// fields on every entry so files and archives past 4 GB are fine.
pub struct ZipWriter<W: Write> {
    out: W,
    at: u64,
    entries: Vec<Entry>,
}

fn dos_time(t: Option<SystemTime>) -> (u16, u16) {
    let secs = t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970 (Howard Hinnant's method).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if y < 1980 {
        return (0, (1 << 5) | 1);
    }
    let time = (((rem / 3600) << 11) | ((rem % 3600 / 60) << 5) | ((rem % 60) / 2)) as u16;
    let date = (((y - 1980) as u16) << 9) | ((m as u16) << 5) | d as u16;
    (time, date)
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out, at: 0, entries: Vec::new() }
    }

    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.out.write_all(b)?;
        self.at += b.len() as u64;
        Ok(())
    }

    /// Adds `len` bytes read from `src` as `name`. The CRC is worked out as
    /// it streams and written in a data descriptor after the bytes.
    pub fn add(&mut self, name: &str, src: &mut impl Read, len: u64, time: Option<SystemTime>) -> io::Result<()> {
        let (dos_time, dos_date) = dos_time(time);
        let offset = self.at;
        let mut h = Vec::with_capacity(30 + name.len() + 20);
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&45u16.to_le_bytes()); // version needed: ZIP64
        h.extend_from_slice(&(0x0008u16 | 0x0800).to_le_bytes()); // data descriptor, UTF-8 names
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&dos_time.to_le_bytes());
        h.extend_from_slice(&dos_date.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes()); // crc: in the descriptor
        h.extend_from_slice(&u32::MAX.to_le_bytes());
        h.extend_from_slice(&u32::MAX.to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        h.extend_from_slice(&0x0001u16.to_le_bytes());
        h.extend_from_slice(&16u16.to_le_bytes());
        h.extend_from_slice(&len.to_le_bytes());
        h.extend_from_slice(&len.to_le_bytes());
        self.put(&h)?;
        let mut crc = crc32fast::Hasher::new();
        let mut left = len;
        let mut buf = vec![0u8; 1 << 20];
        while left > 0 {
            let n = src.read(&mut buf[..left.min(1 << 20) as usize])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{name} got shorter while it was copied")));
            }
            crc.update(&buf[..n]);
            self.put(&buf[..n])?;
            left -= n as u64;
        }
        let crc = crc.finalize();
        let mut d = Vec::with_capacity(24);
        d.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
        d.extend_from_slice(&crc.to_le_bytes());
        d.extend_from_slice(&len.to_le_bytes());
        d.extend_from_slice(&len.to_le_bytes());
        self.put(&d)?;
        self.entries.push(Entry { name: name.to_owned(), crc, size: len, offset, dos_time, dos_date });
        Ok(())
    }

    /// Writes the central directory and hands back the writer.
    pub fn finish(mut self) -> io::Result<W> {
        let start = self.at;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut c = Vec::with_capacity(46 + e.name.len() + 28);
            c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            c.extend_from_slice(&45u16.to_le_bytes());
            c.extend_from_slice(&45u16.to_le_bytes());
            c.extend_from_slice(&(0x0008u16 | 0x0800).to_le_bytes());
            c.extend_from_slice(&0u16.to_le_bytes());
            c.extend_from_slice(&e.dos_time.to_le_bytes());
            c.extend_from_slice(&e.dos_date.to_le_bytes());
            c.extend_from_slice(&e.crc.to_le_bytes());
            c.extend_from_slice(&u32::MAX.to_le_bytes());
            c.extend_from_slice(&u32::MAX.to_le_bytes());
            c.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            c.extend_from_slice(&28u16.to_le_bytes());
            c.extend_from_slice(&[0u8; 6]); // comment len, disk, internal attrs
            c.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            c.extend_from_slice(&u32::MAX.to_le_bytes()); // offset: in ZIP64
            c.extend_from_slice(e.name.as_bytes());
            c.extend_from_slice(&0x0001u16.to_le_bytes());
            c.extend_from_slice(&24u16.to_le_bytes());
            c.extend_from_slice(&e.size.to_le_bytes());
            c.extend_from_slice(&e.size.to_le_bytes());
            c.extend_from_slice(&e.offset.to_le_bytes());
            self.put(&c)?;
        }
        let size = self.at - start;
        let n = entries.len() as u64;
        let z64 = self.at;
        let mut t = Vec::with_capacity(56 + 20 + 22);
        t.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
        t.extend_from_slice(&44u64.to_le_bytes());
        t.extend_from_slice(&45u16.to_le_bytes());
        t.extend_from_slice(&45u16.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&n.to_le_bytes());
        t.extend_from_slice(&n.to_le_bytes());
        t.extend_from_slice(&size.to_le_bytes());
        t.extend_from_slice(&start.to_le_bytes());
        t.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&z64.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        t.extend_from_slice(&[0u8; 4]);
        t.extend_from_slice(&u16::MAX.to_le_bytes());
        t.extend_from_slice(&u16::MAX.to_le_bytes());
        t.extend_from_slice(&u32::MAX.to_le_bytes());
        t.extend_from_slice(&u32::MAX.to_le_bytes());
        t.extend_from_slice(&0u16.to_le_bytes());
        self.put(&t)?;
        Ok(self.out)
    }
}

/// Unpacks a zip written by [`Library::export_zip`] (or any zip of stored
/// entries) into `into`, and returns the library folder found in it.
/// Names that would land outside `into` are refused.
pub fn unzip(zip: &Path, into: &Path) -> Result<PathBuf> {
    let io = |e: io::Error| Error::Io { path: zip.to_owned(), source: e };
    let bad = |why: &str| Error::Backup(format!("{}: {why}", zip.display()));
    let mut f = File::open(zip).at(zip)?;
    let len = f.metadata().at(zip)?.len();
    // The end record is in the last 64 KB + 22 bytes.
    let tail_len = len.min(65_557);
    f.seek(SeekFrom::Start(len - tail_len)).map_err(io)?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail).map_err(io)?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| tail[i..i + 4] == 0x0605_4b50u32.to_le_bytes()).ok_or_else(|| bad("not a zip file"))?;
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let u64_at = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    let (mut count, mut cd_size, mut cd_start) = (u16_at(&tail, eocd + 10) as u64, u32_at(&tail, eocd + 12) as u64, u32_at(&tail, eocd + 16) as u64);
    if eocd >= 20 && tail[eocd - 20..eocd - 16] == 0x0706_4b50u32.to_le_bytes() {
        let z64 = u64_at(&tail, eocd - 12);
        f.seek(SeekFrom::Start(z64)).map_err(io)?;
        let mut r = [0u8; 56];
        f.read_exact(&mut r).map_err(io)?;
        if r[..4] != 0x0606_4b50u32.to_le_bytes() {
            return Err(bad("damaged ZIP64 record"));
        }
        (count, cd_size, cd_start) = (u64_at(&r, 32), u64_at(&r, 40), u64_at(&r, 48));
    }
    if cd_size > 1 << 30 || cd_start + cd_size > len {
        return Err(bad("damaged directory"));
    }
    f.seek(SeekFrom::Start(cd_start)).map_err(io)?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd).map_err(io)?;
    fs::create_dir_all(into).at(into)?;
    let mut at = 0usize;
    let mut library = None;
    for _ in 0..count {
        if at + 46 > cd.len() || cd[at..at + 4] != 0x0201_4b50u32.to_le_bytes() {
            return Err(bad("damaged directory"));
        }
        let method = u16_at(&cd, at + 10);
        let crc = u32_at(&cd, at + 16);
        let (mut csize, mut size, mut offset) = (u32_at(&cd, at + 20) as u64, u32_at(&cd, at + 24) as u64, u32_at(&cd, at + 42) as u64);
        let (nl, el, cl) = (u16_at(&cd, at + 28) as usize, u16_at(&cd, at + 30) as usize, u16_at(&cd, at + 32) as usize);
        let name = String::from_utf8_lossy(cd.get(at + 46..at + 46 + nl).ok_or_else(|| bad("damaged directory"))?).into_owned();
        let mut x = at + 46 + nl;
        let extra_end = (x + el).min(cd.len());
        while x + 4 <= extra_end {
            let (id, l) = (u16_at(&cd, x), u16_at(&cd, x + 2) as usize);
            if id == 1 {
                let mut p = x + 4;
                for v in [&mut size, &mut csize, &mut offset] {
                    if *v == u32::MAX as u64 && p + 8 <= x + 4 + l {
                        *v = u64_at(&cd, p);
                        p += 8;
                    }
                }
            }
            x += 4 + l;
        }
        at += 46 + nl + el + cl;
        if name.ends_with('/') {
            continue;
        }
        if method != 0 || csize != size {
            return Err(bad(&format!("{name} is compressed; only Wall's own exports can be opened here")));
        }
        let parts: Vec<&str> = name.split(['/', '\\']).collect();
        if parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(':')) {
            return Err(bad(&format!("refused the name {name}")));
        }
        let to = parts.iter().fold(into.to_path_buf(), |p, s| p.join(s));
        if parts.last() == Some(&"wall.json") && parts.len() <= 2 {
            library = Some(to.parent().unwrap_or(into).to_owned());
        }
        f.seek(SeekFrom::Start(offset)).map_err(io)?;
        let mut lh = [0u8; 30];
        f.read_exact(&mut lh).map_err(io)?;
        if lh[..4] != 0x0403_4b50u32.to_le_bytes() {
            return Err(bad("damaged entry"));
        }
        let skip = u16_at(&lh, 26) as i64 + u16_at(&lh, 28) as i64;
        f.seek(SeekFrom::Current(skip)).map_err(io)?;
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).at(dir)?;
        }
        let mut out = BufWriter::new(File::create(&to).at(&to)?);
        let mut h = crc32fast::Hasher::new();
        let mut left = size;
        let mut buf = vec![0u8; 1 << 20];
        while left > 0 {
            let n = f.read(&mut buf[..left.min(1 << 20) as usize]).map_err(io)?;
            if n == 0 {
                return Err(bad("cut short"));
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).at(&to)?;
            left -= n as u64;
        }
        out.flush().at(&to)?;
        if h.finalize() != crc {
            return Err(bad(&format!("{name} is damaged")));
        }
    }
    library.ok_or_else(|| bad("no Wall library in it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Board;

    fn library(dir: &Path) -> Library {
        let lib = Library::create(dir.join("Refs.wall"), "Refs", 1).unwrap();
        lib.import_bytes(b"picture one", "png").unwrap();
        lib.import_bytes(&vec![7u8; 3_000_000], "mp4").unwrap();
        lib.write_board(&Board::new("Home", 1)).unwrap();
        fs::create_dir_all(lib.root().join("cache/previews")).unwrap();
        fs::write(lib.root().join("cache/previews/x.png"), b"preview").unwrap();
        lib
    }

    #[test]
    fn a_backup_copies_only_what_changed_and_opens_as_a_library() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());
        let dest = dir.path().join("Backup");
        let r = lib.back_up_to(&dest).unwrap();
        assert_eq!((r.files, r.copied), (4, 4), "wall.json, two media files, one board");
        assert!(!dest.join("cache").exists(), "previews are left out");
        let again = lib.back_up_to(&dest).unwrap();
        assert_eq!(again.copied, 0, "nothing changed");
        let mut b = Board::new("Second", 2);
        b.name = "Second".into();
        lib.write_board(&b).unwrap();
        assert_eq!(lib.back_up_to(&dest).unwrap().copied, 1);
        // Deleting from the library never deletes from the backup.
        lib.trash_board(b.id, 3).unwrap();
        lib.back_up_to(&dest).unwrap();
        assert!(dest.join(format!("boards/{}.json", b.id)).is_file());
        let copy = Library::open(&dest).unwrap();
        assert_eq!(copy.manifest().id, lib.manifest().id);
        assert_eq!(copy.board_ids().unwrap().len(), 2);
    }

    #[test]
    fn a_backup_never_goes_into_the_wrong_folder() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());
        assert!(lib.back_up_to(&lib.root().join("inside")).is_err());
        let other = Library::create(dir.path().join("Other.wall"), "Other", 1).unwrap();
        assert!(lib.back_up_to(other.root()).is_err(), "another library");
        let full = dir.path().join("Full");
        fs::create_dir_all(&full).unwrap();
        fs::write(full.join("holiday.jpg"), b"x").unwrap();
        assert!(lib.back_up_to(&full).is_err(), "a folder with other things in it");
    }

    #[test]
    fn an_export_unzips_into_the_same_library() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library(dir.path());
        let zip = dir.path().join("Refs.zip");
        let r = lib.export_zip(&zip).unwrap();
        assert_eq!(r.files, 4);
        let back = unzip(&zip, &dir.path().join("Restored")).unwrap();
        assert_eq!(back, dir.path().join("Restored/Refs.wall"));
        let copy = Library::open(&back).unwrap();
        assert_eq!(copy.manifest().id, lib.manifest().id);
        for (rel, from) in lib.files().unwrap() {
            assert_eq!(fs::read(back.join(&rel)).unwrap(), fs::read(from).unwrap(), "{rel}");
        }
        // Other zip tools read it too, where one is installed.
        if let Ok(out) = std::process::Command::new("unzip").arg("-tq").arg(&zip).output() {
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        }
        // A damaged byte is noticed.
        let mut bytes = fs::read(&zip).unwrap();
        let at = bytes.windows(4).position(|w| w == [7, 7, 7, 7]).unwrap();
        bytes[at] = 8;
        fs::write(&zip, bytes).unwrap();
        assert!(unzip(&zip, &dir.path().join("Again")).is_err());
    }

    #[test]
    fn dos_dates() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_795_000_000); // 2026-11-18 11:06:40 UTC
        let (time, date) = dos_time(Some(t));
        assert_eq!((date >> 9) + 1980, 2026);
        assert_eq!((date >> 5) & 15, 11);
        assert_eq!(date & 31, 18);
        assert_eq!((time >> 11, (time >> 5) & 63), (11, 6));
    }
}
