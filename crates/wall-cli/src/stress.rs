//! `wall-cli stress`: a test board with many pictures, videos and notes, to
//! see how Wall copes with big boards. Pictures are made here; videos need
//! ffmpeg. Everything lands on a new board in a grid.

use std::path::Path;

use wall_format::{Board, BoardItem, Frame, ItemKind, Library};
use wall_import::media::import_one;
use wall_import::Res;

pub struct Counts {
    pub pictures: usize,
    pub videos: usize,
    pub notes: usize,
}

/// Parses `--pictures N --videos N --notes N` (any order, any left out).
pub fn counts(args: &[String]) -> Res<Counts> {
    let mut c = Counts { pictures: 500, videos: 100, notes: 200 };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let n: usize = it.next().and_then(|v| v.parse().ok()).ok_or_else(|| format!("{a} needs a number"))?;
        match a.as_str() {
            "--pictures" => c.pictures = n,
            "--videos" => c.videos = n,
            "--notes" => c.notes = n,
            _ => return Err(format!("unknown option {a}")),
        }
    }
    Ok(c)
}

/// A picture that differs from every other one (so each is its own file).
fn picture(i: usize) -> Vec<u8> {
    let (w, h) = [(800u32, 600u32), (600, 900), (1200, 675), (700, 700)][i % 4];
    let hue = (i as f32 * 0.137).fract();
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let u = x as f32 / w as f32;
        let v = y as f32 / h as f32;
        let band = (((x / 40) + (y / 40)) % 2) as f32 * 0.12;
        let r = ((hue + u * 0.3).fract() * 255.0 * (1.0 - v * 0.5) + band * 255.0).min(255.0) as u8;
        let g = (((hue + 0.33 + v * 0.2).fract()) * 200.0) as u8;
        let b = ((((hue + 0.66).fract()) * 255.0) * (0.5 + u * 0.5)) as u8;
        // A few pixels from the index, so no two pictures are the same.
        let id = if y < 8 && x < 64 { ((i >> (x / 8)) & 1) as u8 * 255 } else { 0 };
        image::Rgb([r ^ id, g, b])
    });
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).expect("jpeg");
    out
}

/// A short test video, different for each `i`.
fn video(dir: &Path, i: usize) -> Res<std::path::PathBuf> {
    use wall_media::ffmpeg;
    let exe = ffmpeg::ffmpeg().ok_or("videos need ffmpeg")?;
    let path = dir.join(format!("stress-{i}.mp4"));
    let (w, h) = if i.is_multiple_of(3) { (480, 640) } else { (640, 360) };
    let secs = 2 + i % 4;
    let src = format!("testsrc2=size={w}x{h}:rate=30:duration={secs}");
    let out = ffmpeg::command(exe)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", &src])
        .args(["-vf", &format!("hue=h={}", (i * 37) % 360)])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-g", "30", "-metadata", &format!("title=stress {i}")])
        .arg(&path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(path)
}

pub fn run(lib: &Library, c: &Counts, now: u64) -> Res<String> {
    let tmp = std::env::temp_dir().join(format!("wall-stress-{now}"));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut board = Board::new(format!("Stress test: {} pictures, {} videos, {} notes", c.pictures, c.videos, c.notes), now);
    board.parent = lib.board_ids().ok().and_then(|ids| ids.into_iter().filter_map(|id| lib.read_board(id).ok()).find(|b| b.parent.is_none()).map(|b| b.id));
    let total = c.pictures + c.videos + c.notes;
    let cols = (total as f64).sqrt().ceil().max(1.0) as usize;
    let mut skipped = Vec::new();
    let put = |board: &mut Board, k: usize, kind: ItemKind, w: f64, h: f64| {
        let (cx, cy) = ((k % cols) as f64 * 360.0, (k / cols) as f64 * 360.0);
        let id = uuid::Uuid::new_v4();
        board.items.insert(id, BoardItem { id, frame: Frame { x: cx, y: cy, w, h, rotation: 0.0, z: k as f64, text_scale: 1.0 }, kind, rev: 1, updated_ms: now, updated_by: "wall-cli".into(), deleted: false });
    };
    // Mixed in, the way a real board is.
    let mut order: Vec<u8> = std::iter::repeat_n(0u8, c.pictures).chain(std::iter::repeat_n(1, c.videos)).chain(std::iter::repeat_n(2, c.notes)).collect();
    let mut seed = 0x9e37_79b9_u64;
    for i in (1..order.len()).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        order.swap(i, (seed >> 33) as usize % (i + 1));
    }
    let (mut p, mut v, mut n) = (0, 0, 0);
    for (k, what) in order.into_iter().enumerate() {
        let file = match what {
            0 => {
                p += 1;
                let f = tmp.join(format!("stress-{p}.jpg"));
                std::fs::write(&f, picture(p)).map_err(|e| e.to_string())?;
                f
            }
            1 => {
                v += 1;
                match video(&tmp, v) {
                    Ok(f) => f,
                    Err(e) => {
                        skipped.push(e);
                        continue;
                    }
                }
            }
            _ => {
                n += 1;
                put(&mut board, k, ItemKind::Note { text: format!("Note {n}: a few words to wrap across lines and check text stays quick."), color: None }, 320.0, 160.0);
                continue;
            }
        };
        let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("file").to_owned();
        match import_one(lib, &file, &name, now) {
            Ok((hash, w, h)) => {
                let s = 320.0 / w.max(h) as f64;
                put(&mut board, k, ItemKind::Media { hash, crop: None, look: Default::default() }, w as f64 * s, h as f64 * s);
            }
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
        let _ = std::fs::remove_file(&file);
        if k % 50 == 0 {
            eprint!("\r{k}/{total}");
        }
    }
    eprintln!();
    let _ = std::fs::remove_dir_all(&tmp);
    lib.write_board(&board).map_err(|e| e.to_string())?;
    let mut out = format!("made \"{}\" ({}) with {} items\n", board.name, board.id, board.items.len());
    for s in skipped.iter().take(5) {
        out.push_str(&format!("skipped {s}\n"));
    }
    Ok(out)
}
