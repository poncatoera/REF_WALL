//! Just enough MP4 to shrink a video in the browser: read the sample table of
//! an MP4 or MOV file, and write an H.264 MP4 from encoded frames.
//!
//! The web build reads the file in slices (a 350 MB video never sits in
//! memory whole), decodes and re-encodes the frames with WebCodecs, and
//! writes the result with [`Mp4Writer`]. Only video is kept: Wall plays
//! videos muted.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mp4Error {
    /// A box runs past the end of its parent, or a table is cut short.
    Truncated(&'static str),
    Missing(&'static str),
    /// The file has no video track Wall can hand to a browser decoder.
    NoVideo,
    Unsupported(String),
}

impl std::fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mp4Error::Truncated(what) => write!(f, "the file is damaged ({what} is cut short)"),
            Mp4Error::Missing(what) => write!(f, "the file has no {what}"),
            Mp4Error::NoVideo => write!(f, "the file has no video Wall can read"),
            Mp4Error::Unsupported(what) => write!(f, "{what} isn't supported"),
        }
    }
}

impl std::error::Error for Mp4Error {}

/// A top-level box header, read from the first 16 bytes at some offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxHeader {
    pub kind: [u8; 4],
    /// Header length: 8, or 16 with a 64-bit size.
    pub header_len: u64,
    /// Whole box size including the header; `None` runs to the end of the file.
    pub size: Option<u64>,
}

/// Reads a box header. `bytes` may be shorter than 16 when the box isn't
/// 64-bit sized.
pub fn box_header(bytes: &[u8]) -> Result<BoxHeader, Mp4Error> {
    if bytes.len() < 8 {
        return Err(Mp4Error::Truncated("a box header"));
    }
    let size32 = be32(bytes, 0);
    let kind = [bytes[4], bytes[5], bytes[6], bytes[7]];
    match size32 {
        0 => Ok(BoxHeader { kind, header_len: 8, size: None }),
        1 => {
            if bytes.len() < 16 {
                return Err(Mp4Error::Truncated("a box header"));
            }
            let size = be64(bytes, 8);
            if size < 16 {
                return Err(Mp4Error::Truncated("a box header"));
            }
            Ok(BoxHeader { kind, header_len: 16, size: Some(size) })
        }
        s if s < 8 => Err(Mp4Error::Truncated("a box header")),
        s => Ok(BoxHeader { kind, header_len: 8, size: Some(s as u64) }),
    }
}

/// One coded frame in the source file. Times are in the track's timescale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub offset: u64,
    pub size: u32,
    /// Presentation time.
    pub cts: i64,
    /// Decode time.
    pub dts: i64,
    pub key: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoTrack {
    /// Sample entry type, e.g. `avc1`, `hvc1`, `av01`.
    pub fourcc: String,
    /// The codec string WebCodecs wants, e.g. `avc1.64001F`.
    pub codec: String,
    /// The codec configuration (avcC / hvcC / av1C / vpcC payload) for
    /// `VideoDecoderConfig.description`; empty for VP9.
    pub description: Vec<u8>,
    /// Coded size, before [`rotation`](Self::rotation).
    pub width: u32,
    pub height: u32,
    /// Clockwise turn (0, 90, 180 or 270) that shows the video upright;
    /// phones film portrait as landscape plus a turn.
    pub rotation: u32,
    pub timescale: u32,
    /// Samples in decode order.
    pub samples: Vec<Sample>,
}

impl VideoTrack {
    /// Size as shown, after the turn.
    pub fn upright_size(&self) -> (u32, u32) {
        if self.rotation % 180 == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    pub fn duration_ms(&self) -> u64 {
        let end = self.samples.iter().map(|s| s.cts).max().unwrap_or(0);
        let start = self.samples.iter().map(|s| s.cts).min().unwrap_or(0);
        // Add one frame for the last one's display time.
        let frame = if self.samples.len() > 1 { (end - start) / (self.samples.len() as i64 - 1) } else { 0 };
        ((end - start + frame).max(0) as u64) * 1000 / self.timescale.max(1) as u64
    }

    /// Longest gap between keyframes, for `VideoProbe::max_keyframe_gap_ms`.
    pub fn max_keyframe_gap_ms(&self) -> u64 {
        let mut keys: Vec<i64> = self.samples.iter().filter(|s| s.key).map(|s| s.cts).collect();
        keys.sort_unstable();
        let last = self.samples.iter().map(|s| s.cts).max().unwrap_or(0);
        keys.push(last);
        let gap = keys.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
        gap.max(0) as u64 * 1000 / self.timescale.max(1) as u64
    }
}

/// Finds the first video track in a `moov` box (the whole box, header included).
pub fn parse_moov(moov: &[u8]) -> Result<VideoTrack, Mp4Error> {
    let body = body_of(moov, b"moov")?;
    let mut last_err = Mp4Error::NoVideo;
    for (kind, trak) in children(body)? {
        if &kind != b"trak" {
            continue;
        }
        match parse_trak(trak) {
            Ok(Some(t)) => return Ok(t),
            Ok(None) => {}
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

fn parse_trak(trak: &[u8]) -> Result<Option<VideoTrack>, Mp4Error> {
    let mdia = child(trak, b"mdia")?.ok_or(Mp4Error::Missing("media box"))?;
    let hdlr = child(mdia, b"hdlr")?.ok_or(Mp4Error::Missing("handler box"))?;
    if hdlr.len() < 12 || &hdlr[8..12] != b"vide" {
        return Ok(None);
    }
    let mdhd = child(mdia, b"mdhd")?.ok_or(Mp4Error::Missing("media header"))?;
    let timescale = match mdhd.first() {
        Some(1) => need(mdhd, 24, "media header").map(|_| be32(mdhd, 20))?,
        _ => need(mdhd, 16, "media header").map(|_| be32(mdhd, 12))?,
    };
    if timescale == 0 {
        return Err(Mp4Error::Truncated("media header"));
    }
    let minf = child(mdia, b"minf")?.ok_or(Mp4Error::Missing("media info"))?;
    let stbl = child(minf, b"stbl")?.ok_or(Mp4Error::Missing("sample table"))?;

    let stsd = child(stbl, b"stsd")?.ok_or(Mp4Error::Missing("sample description"))?;
    need(stsd, 8 + 8 + 78, "sample description")?;
    let entry_hdr = box_header(&stsd[8..])?;
    let entry_size = entry_hdr.size.ok_or(Mp4Error::Truncated("sample description"))? as usize;
    let entry = stsd.get(8..8 + entry_size).ok_or(Mp4Error::Truncated("sample description"))?;
    let fourcc = String::from_utf8_lossy(&entry[4..8]).into_owned();
    let width = be16(entry, 8 + 24) as u32;
    let height = be16(entry, 8 + 26) as u32;
    let extras = &entry[8 + 78..];
    let (codec, description) = codec_of(&fourcc, extras)?;

    let stts = table(stbl, b"stts", 8)?.ok_or(Mp4Error::Missing("sample times"))?;
    let ctts = table(stbl, b"ctts", 8)?;
    let stss = table(stbl, b"stss", 4)?;
    let stsc = table(stbl, b"stsc", 12)?.ok_or(Mp4Error::Missing("sample chunks"))?;
    let stsz = child(stbl, b"stsz")?.ok_or_else(|| match child(stbl, b"stz2") {
        Ok(Some(_)) => Mp4Error::Unsupported("compact sample sizes (stz2)".into()),
        _ => Mp4Error::Missing("sample sizes"),
    })?;
    need(stsz, 12, "sample sizes")?;
    let fixed_size = be32(stsz, 4);
    let count = be32(stsz, 8) as usize;
    if fixed_size == 0 {
        need(stsz, 12 + count * 4, "sample sizes")?;
    }
    let size_of = |i: usize| if fixed_size != 0 { fixed_size } else { be32(stsz, 12 + i * 4) };

    let chunk_offsets: Vec<u64> = if let Some(stco) = table(stbl, b"stco", 4)? {
        stco.chunks_exact(4).map(|c| be32(c, 0) as u64).collect()
    } else if let Some(co64) = table(stbl, b"co64", 8)? {
        co64.chunks_exact(8).map(|c| be64(c, 0)).collect()
    } else {
        return Err(Mp4Error::Missing("chunk offsets"));
    };

    // Decode times.
    let mut dts = Vec::with_capacity(count);
    let mut t: i64 = 0;
    for run in stts.chunks_exact(8) {
        let (n, delta) = (be32(run, 0), be32(run, 4));
        for _ in 0..n {
            if dts.len() == count {
                break;
            }
            dts.push(t);
            t += delta as i64;
        }
    }
    while dts.len() < count {
        dts.push(t);
    }
    // Composition offsets (signed in practice for version 1, and also used so by some v0 writers).
    let mut cts_off = vec![0i64; count];
    if let Some(ctts) = ctts {
        let mut i = 0;
        for run in ctts.chunks_exact(8) {
            let (n, off) = (be32(run, 0), be32(run, 4) as i32 as i64);
            for _ in 0..n {
                if i == count {
                    break;
                }
                cts_off[i] = off;
                i += 1;
            }
        }
    }
    let keys: Option<std::collections::HashSet<u32>> = stss.map(|s| s.chunks_exact(4).map(|c| be32(c, 0)).collect());

    // Sample offsets from the chunk map.
    let runs: Vec<(u32, u32)> = stsc.chunks_exact(12).map(|r| (be32(r, 0), be32(r, 4))).collect();
    let mut samples = Vec::with_capacity(count);
    'chunks: for (ci, &chunk_offset) in chunk_offsets.iter().enumerate() {
        let chunk_no = ci as u32 + 1;
        let per_chunk = runs.iter().rev().find(|(first, _)| *first <= chunk_no).map(|r| r.1).unwrap_or(0);
        let mut offset = chunk_offset;
        for _ in 0..per_chunk {
            let i = samples.len();
            if i == count {
                break 'chunks;
            }
            let size = size_of(i);
            samples.push(Sample {
                offset,
                size,
                dts: dts[i],
                cts: dts[i] + cts_off[i],
                key: keys.as_ref().is_none_or(|k| k.contains(&(i as u32 + 1))),
            });
            offset += size as u64;
        }
    }
    if samples.len() < count {
        return Err(Mp4Error::Truncated("chunk map"));
    }
    if samples.is_empty() {
        return Err(Mp4Error::NoVideo);
    }
    let rotation = match child(trak, b"tkhd")? {
        Some(tkhd) => rotation_of(tkhd),
        None => 0,
    };
    Ok(Some(VideoTrack { fourcc, codec, description, width, height, rotation, timescale, samples }))
}

/// The turn in a track header's matrix (a b / c d in 16.16 fixed point).
fn rotation_of(tkhd: &[u8]) -> u32 {
    let at = if tkhd.first() == Some(&1) { 52 } else { 40 };
    if tkhd.len() < at + 16 {
        return 0;
    }
    let m = |i: usize| be32(tkhd, at + i * 4) as i32;
    let sign = |v: i32| v.signum();
    match (sign(m(0)), sign(m(1)), sign(m(3)), sign(m(4))) {
        (0, 1, -1, 0) => 90,
        (-1, 0, 0, -1) => 180,
        (0, -1, 1, 0) => 270,
        _ => 0,
    }
}

/// The WebCodecs codec string and decoder description for a sample entry.
fn codec_of(fourcc: &str, extras: &[u8]) -> Result<(String, Vec<u8>), Mp4Error> {
    let find = |kind: &[u8; 4]| -> Result<Vec<u8>, Mp4Error> {
        child(extras, kind)?.map(<[u8]>::to_vec).ok_or(Mp4Error::Missing("codec settings"))
    };
    match fourcc {
        "avc1" | "avc3" => {
            let c = find(b"avcC")?;
            need(&c, 4, "codec settings")?;
            Ok((format!("{fourcc}.{:02X}{:02X}{:02X}", c[1], c[2], c[3]), c))
        }
        "hvc1" | "hev1" => {
            let c = find(b"hvcC")?;
            Ok((hevc_codec(fourcc, &c)?, c))
        }
        "av01" => {
            let c = find(b"av1C")?;
            need(&c, 3, "codec settings")?;
            let profile = c[1] >> 5;
            let level = c[1] & 0x1f;
            let tier = if c[2] & 0x80 != 0 { 'H' } else { 'M' };
            let depth = if c[2] & 0x40 == 0 { 8 } else if c[2] & 0x20 != 0 { 12 } else { 10 };
            Ok((format!("av01.{profile}.{level:02}{tier}.{depth:02}"), c))
        }
        "vp09" => {
            let c = find(b"vpcC")?;
            need(&c, 7, "codec settings")?;
            Ok((format!("vp09.{:02}.{:02}.{:02}", c[4], c[5], c[6] >> 4), Vec::new()))
        }
        other => Err(Mp4Error::Unsupported(format!("video type \"{}\"", other.trim()))),
    }
}

/// `hvc1.1.6.L93.B0` style string (ISO/IEC 14496-15 annex E).
fn hevc_codec(fourcc: &str, c: &[u8]) -> Result<String, Mp4Error> {
    need(c, 13, "codec settings")?;
    let space = ["", "A", "B", "C"][(c[1] >> 6) as usize];
    let tier = if c[1] & 0x20 != 0 { 'H' } else { 'L' };
    let profile = c[1] & 0x1f;
    let compat = be32(c, 2).reverse_bits();
    let level = c[12];
    let mut s = format!("{fourcc}.{space}{profile}.{compat:X}.{tier}{level}");
    let constraints = &c[6..12];
    let used = constraints.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    for b in &constraints[..used] {
        let _ = write!(s, ".{b:X}");
    }
    Ok(s)
}

/// Writes an MP4 holding one H.264 video track, `moov` first so it starts
/// playing before it has fully loaded.
#[derive(Debug, Clone)]
pub struct Mp4Writer {
    avcc: Vec<u8>,
    width: u32,
    height: u32,
    mdat: Vec<u8>,
    sizes: Vec<u32>,
    /// Presentation times in microseconds, in the order pushed.
    times_us: Vec<i64>,
    keys: Vec<u32>,
}

/// Ticks per second in the written file.
const TIMESCALE: u32 = 90_000;

impl Mp4Writer {
    /// `avcc` is the encoder's `decoderConfig.description`.
    pub fn new(avcc: Vec<u8>, width: u32, height: u32) -> Self {
        Self { avcc, width, height, mdat: Vec::new(), sizes: Vec::new(), times_us: Vec::new(), keys: Vec::new() }
    }

    pub fn set_avcc(&mut self, avcc: Vec<u8>) {
        self.avcc = avcc;
    }

    /// Adds one encoded frame (length-prefixed NAL units, as WebCodecs gives
    /// them with `avc: { format: "avc" }`). Frames come in decode order, which
    /// for an encoder without B-frames is presentation order.
    pub fn push(&mut self, data: &[u8], time_us: i64, key: bool) {
        self.mdat.extend_from_slice(data);
        self.sizes.push(data.len() as u32);
        self.times_us.push(time_us);
        if key {
            self.keys.push(self.sizes.len() as u32);
        }
    }

    pub fn frames(&self) -> usize {
        self.sizes.len()
    }

    pub fn bytes_so_far(&self) -> usize {
        self.mdat.len()
    }

    /// The finished file. `last_duration_us` is how long the last frame shows.
    pub fn finish(self, last_duration_us: i64) -> Vec<u8> {
        let ticks = |us: i64| (us as i128 * TIMESCALE as i128 / 1_000_000) as i64;
        let start = self.times_us.first().copied().unwrap_or(0);
        let mut durations: Vec<u32> = self.times_us.windows(2).map(|w| (ticks(w[1] - start) - ticks(w[0] - start)).max(1) as u32).collect();
        if !self.sizes.is_empty() {
            durations.push(ticks(last_duration_us).max(1) as u32);
        }
        let total: u64 = durations.iter().map(|&d| d as u64).sum();
        let total_ms = total * 1000 / TIMESCALE as u64;

        let mut stts = Vec::new();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &d in &durations {
            match runs.last_mut() {
                Some((n, delta)) if *delta == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        put32(&mut stts, runs.len() as u32);
        for (n, d) in runs {
            put32(&mut stts, n);
            put32(&mut stts, d);
        }
        let mut stss = Vec::new();
        put32(&mut stss, self.keys.len() as u32);
        for &k in &self.keys {
            put32(&mut stss, k);
        }
        let mut stsc = Vec::new();
        put32(&mut stsc, 1);
        put32(&mut stsc, 1);
        put32(&mut stsc, self.sizes.len() as u32);
        put32(&mut stsc, 1);
        let mut stsz = Vec::new();
        put32(&mut stsz, 0);
        put32(&mut stsz, self.sizes.len() as u32);
        for &s in &self.sizes {
            put32(&mut stsz, s);
        }

        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(b"isom");
        put32(&mut ftyp, 0x200);
        for b in [b"isom", b"iso2", b"avc1", b"mp41"] {
            ftyp.extend_from_slice(b);
        }
        let ftyp = mp4_box(b"ftyp", &ftyp);

        // The chunk offset depends on the moov size, which doesn't depend on
        // the offset's value: build once with 0, then patch.
        let build = |chunk_offset: u32| -> Vec<u8> {
            let mut stco = Vec::new();
            put32(&mut stco, 1);
            put32(&mut stco, chunk_offset);
            let stbl = [
                full_box(b"stsd", 0, &self.sample_entry()),
                full_box(b"stts", 0, &stts),
                full_box(b"stss", 0, &stss),
                full_box(b"stsc", 0, &stsc),
                full_box(b"stsz", 0, &stsz),
                full_box(b"stco", 0, &stco),
            ]
            .concat();
            let mut dref = Vec::new();
            put32(&mut dref, 1);
            dref.extend_from_slice(&full_box_flags(b"url ", 0, 1, &[]));
            let minf = [full_box_flags(b"vmhd", 0, 1, &[0; 8]), mp4_box(b"dinf", &full_box(b"dref", 0, &dref)), mp4_box(b"stbl", &stbl)].concat();
            let mut mdhd = Vec::new();
            put32(&mut mdhd, 0);
            put32(&mut mdhd, 0);
            put32(&mut mdhd, TIMESCALE);
            put32(&mut mdhd, total.min(u32::MAX as u64) as u32);
            mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]); // "und"
            let mut hdlr = Vec::new();
            put32(&mut hdlr, 0);
            hdlr.extend_from_slice(b"vide");
            hdlr.extend_from_slice(&[0; 12]);
            hdlr.extend_from_slice(b"VideoHandler\0");
            let mdia = [full_box(b"mdhd", 0, &mdhd), full_box(b"hdlr", 0, &hdlr), mp4_box(b"minf", &minf)].concat();
            let mut tkhd = Vec::new();
            put32(&mut tkhd, 0);
            put32(&mut tkhd, 0);
            put32(&mut tkhd, 1); // track id
            put32(&mut tkhd, 0);
            put32(&mut tkhd, total_ms as u32);
            tkhd.extend_from_slice(&[0; 8]);
            tkhd.extend_from_slice(&[0; 8]); // layer, group, volume, reserved
            put_matrix(&mut tkhd);
            put32(&mut tkhd, self.width << 16);
            put32(&mut tkhd, self.height << 16);
            let trak = [full_box_flags(b"tkhd", 0, 3, &tkhd), mp4_box(b"mdia", &mdia)].concat();
            let mut mvhd = Vec::new();
            put32(&mut mvhd, 0);
            put32(&mut mvhd, 0);
            put32(&mut mvhd, 1000);
            put32(&mut mvhd, total_ms as u32);
            put32(&mut mvhd, 0x0001_0000); // rate
            mvhd.extend_from_slice(&[0x01, 0x00]); // volume
            mvhd.extend_from_slice(&[0; 10]);
            put_matrix(&mut mvhd);
            mvhd.extend_from_slice(&[0; 24]);
            put32(&mut mvhd, 2); // next track id
            mp4_box(b"moov", &[full_box(b"mvhd", 0, &mvhd), mp4_box(b"trak", &trak)].concat())
        };
        let moov_len = build(0).len();
        let mdat_header = 8;
        let moov = build((ftyp.len() + moov_len + mdat_header) as u32);

        let mut out = Vec::with_capacity(ftyp.len() + moov.len() + mdat_header + self.mdat.len());
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        put32(&mut out, (self.mdat.len() + mdat_header) as u32);
        out.extend_from_slice(b"mdat");
        out.extend_from_slice(&self.mdat);
        out
    }

    fn sample_entry(&self) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend_from_slice(&[0; 6]);
        e.extend_from_slice(&[0, 1]); // data reference index
        e.extend_from_slice(&[0; 16]);
        e.extend_from_slice(&(self.width as u16).to_be_bytes());
        e.extend_from_slice(&(self.height as u16).to_be_bytes());
        put32(&mut e, 0x0048_0000);
        put32(&mut e, 0x0048_0000);
        put32(&mut e, 0);
        e.extend_from_slice(&[0, 1]); // frame count
        e.extend_from_slice(&[0; 32]);
        e.extend_from_slice(&[0x00, 0x18, 0xff, 0xff]);
        e.extend_from_slice(&mp4_box(b"avcC", &self.avcc));
        let mut stsd = Vec::new();
        put32(&mut stsd, 1);
        stsd.extend_from_slice(&mp4_box(b"avc1", &e));
        stsd
    }
}

// ---- byte helpers -------------------------------------------------------

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be64(b: &[u8], at: usize) -> u64 {
    let mut a = [0; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_be_bytes(a)
}

fn need(b: &[u8], len: usize, what: &'static str) -> Result<(), Mp4Error> {
    if b.len() < len {
        Err(Mp4Error::Truncated(what))
    } else {
        Ok(())
    }
}

/// The body of a whole box, checking its type.
fn body_of<'a>(whole: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], Mp4Error> {
    let h = box_header(whole)?;
    if &h.kind != kind {
        return Err(Mp4Error::Missing("movie box"));
    }
    let end = h.size.map_or(whole.len(), |s| s as usize);
    whole.get(h.header_len as usize..end).ok_or(Mp4Error::Truncated("movie box"))
}

/// The child boxes of a box body, as (type, body).
fn children(body: &[u8]) -> Result<Vec<([u8; 4], &[u8])>, Mp4Error> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= body.len() {
        let h = box_header(&body[at..])?;
        let size = h.size.map_or(body.len() - at, |s| s as usize);
        let inner = body.get(at + h.header_len as usize..at + size).ok_or(Mp4Error::Truncated("a box"))?;
        out.push((h.kind, inner));
        at += size;
    }
    Ok(out)
}

fn child<'a>(body: &'a [u8], kind: &[u8; 4]) -> Result<Option<&'a [u8]>, Mp4Error> {
    Ok(children(body)?.into_iter().find(|(k, _)| k == kind).map(|(_, b)| b))
}

/// A full box holding `count` then `count` entries of `entry` bytes; returns the entries.
fn table<'a>(stbl: &'a [u8], kind: &[u8; 4], entry: usize) -> Result<Option<&'a [u8]>, Mp4Error> {
    let Some(b) = child(stbl, kind)? else { return Ok(None) };
    need(b, 8, "sample table")?;
    let n = be32(b, 4) as usize;
    let end = n.checked_mul(entry).and_then(|l| l.checked_add(8)).ok_or(Mp4Error::Truncated("sample table"))?;
    b.get(8..end).map(Some).ok_or(Mp4Error::Truncated("sample table"))
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn put_matrix(v: &mut Vec<u8>) {
    for x in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000u32] {
        put32(v, x);
    }
}

fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    put32(&mut v, (body.len() + 8) as u32);
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn full_box(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
    full_box_flags(kind, version, 0, body)
}

fn full_box_flags(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    put32(&mut b, (version as u32) << 24 | (flags & 0x00ff_ffff));
    b.extend_from_slice(body);
    mp4_box(kind, &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plausible avcC: version 1, High profile, level 3.1, one SPS and PPS.
    fn avcc() -> Vec<u8> {
        vec![1, 0x64, 0x00, 0x1f, 0xff, 0xe1, 0, 4, 0x67, 0x64, 0x00, 0x1f, 1, 0, 2, 0x68, 0xee]
    }

    /// Splits a written file into its top-level boxes, as the web reader does.
    fn top_level(file: &[u8]) -> Vec<([u8; 4], std::ops::Range<usize>)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < file.len() {
            let h = box_header(&file[at..]).unwrap();
            let end = h.size.map_or(file.len(), |s| at + s as usize);
            out.push((h.kind, at..end));
            at = end;
        }
        out
    }

    #[test]
    fn a_written_file_reads_back_with_the_same_frames_times_and_keyframes() {
        let mut w = Mp4Writer::new(avcc(), 640, 360);
        for i in 0..30i64 {
            let frame = vec![i as u8; 100 + i as usize];
            w.push(&frame, i * 33_333, i % 10 == 0);
        }
        let file = w.finish(33_333);
        let boxes = top_level(&file);
        let kinds: Vec<&[u8; 4]> = boxes.iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, [b"ftyp", b"moov", b"mdat"]);

        let moov = &file[boxes[1].1.clone()];
        let t = parse_moov(moov).unwrap();
        assert_eq!(t.codec, "avc1.64001F");
        assert_eq!(t.description, avcc());
        assert_eq!((t.width, t.height), (640, 360));
        assert_eq!(t.samples.len(), 30);
        for (i, s) in t.samples.iter().enumerate() {
            let data = &file[s.offset as usize..s.offset as usize + s.size as usize];
            assert_eq!(data, &vec![i as u8; 100 + i][..]);
            assert_eq!(s.key, i % 10 == 0);
            assert_eq!(s.cts, s.dts);
        }
        assert!((990..=1010).contains(&t.duration_ms()), "{}", t.duration_ms());
        assert!((320..=340).contains(&t.max_keyframe_gap_ms()), "{}", t.max_keyframe_gap_ms());
    }

    #[test]
    fn box_headers_read_short_long_and_to_the_end_sizes() {
        assert_eq!(box_header(b"\0\0\0\x10moov").unwrap(), BoxHeader { kind: *b"moov", header_len: 8, size: Some(16) });
        let long = [&[0, 0, 0, 1][..], b"mdat", &(5_000_000_000u64).to_be_bytes()].concat();
        assert_eq!(box_header(&long).unwrap().size, Some(5_000_000_000));
        assert_eq!(box_header(b"\0\0\0\0mdat").unwrap().size, None);
        assert!(box_header(b"\0\0\0\x04free").is_err());
        assert!(box_header(b"\0\0").is_err());
    }

    #[test]
    fn codec_strings_follow_the_registrations() {
        // iPhone HEVC: Main profile, level 4.0 (120 / 3 = 40 → "L120"), progressive source flag.
        let mut hvcc = vec![1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 120];
        hvcc.extend_from_slice(&[0; 10]);
        assert_eq!(hevc_codec("hvc1", &hvcc).unwrap(), "hvc1.1.6.L120.90");
        let av1c = [0x81, 0x08, 0x0c, 0x00];
        let av1 = mp4_box(b"av1C", &av1c);
        assert_eq!(codec_of("av01", &av1).unwrap().0, "av01.0.08M.08");
        assert!(matches!(codec_of("mp4v", &[]), Err(Mp4Error::Unsupported(_))));
    }
}
