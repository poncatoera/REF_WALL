//! Tags Wall can work out on its own, without a network: main colours,
//! shape and video length. They are stored as `AutoTag`s with a `local:`
//! source, apart from the tags people type, so re-running never overwrites
//! what someone chose.

use wall_format::AutoTag;

/// Up to `n` main colours of an RGBA image as `#rrggbb`, most common first.
/// Colours that cover less than 6% of the picture are left out.
pub fn dominant_colors(rgba: &[u8], width: u32, height: u32, n: usize) -> Vec<String> {
    let pixels = (width as usize) * (height as usize);
    if pixels == 0 || rgba.len() < pixels * 4 {
        return Vec::new();
    }
    // Sample about 4,000 pixels, bucketed at 3 bits per channel.
    let step = (pixels / 4000).max(1);
    let mut buckets = vec![(0u32, [0u64; 3]); 512];
    let mut total = 0u32;
    for p in (0..pixels).step_by(step) {
        let px = &rgba[p * 4..p * 4 + 4];
        if px[3] < 128 {
            continue;
        }
        let key = ((px[0] as usize >> 5) << 6) | ((px[1] as usize >> 5) << 3) | (px[2] as usize >> 5);
        let b = &mut buckets[key];
        b.0 += 1;
        for (sum, v) in b.1.iter_mut().zip(px) {
            *sum += *v as u64;
        }
        total += 1;
    }
    if total == 0 {
        return Vec::new();
    }
    let mut ranked: Vec<(u32, [u8; 3])> = buckets
        .iter()
        .filter(|b| b.0 > 0)
        .map(|b| (b.0, [0, 1, 2].map(|c| (b.1[c] / b.0 as u64) as u8)))
        .collect();
    ranked.sort_by_key(|r| std::cmp::Reverse(r.0));
    // Merge colours that look alike, so one gradient doesn't fill every slot.
    let mut out: Vec<(u32, [u8; 3])> = Vec::new();
    for (count, rgb) in ranked {
        match out.iter_mut().find(|(_, o)| distance(*o, rgb) < 60.0) {
            Some(o) => o.0 += count,
            None => out.push((count, rgb)),
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.0));
    out.into_iter()
        .filter(|(c, _)| *c as f32 / total as f32 >= 0.06)
        .take(n)
        .map(|(_, [r, g, b])| format!("#{r:02x}{g:02x}{b:02x}"))
        .collect()
}

fn distance(a: [u8; 3], b: [u8; 3]) -> f32 {
    let d = [0, 1, 2].map(|i| a[i] as f32 - b[i] as f32);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// A plain colour name for `#rrggbb`: red, orange, yellow, green, teal,
/// blue, purple, pink, brown, black, white or grey.
pub fn color_name(hex: &str) -> Option<&'static str> {
    let h = hex.strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    let (r, g, b) = (((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let s = if max == min { 0.0 } else { (max - min) / (1.0 - (2.0 * l - 1.0).abs()) };
    if l < 0.12 {
        return Some("black");
    }
    if l > 0.9 {
        return Some("white");
    }
    if s < 0.18 {
        return Some("grey");
    }
    let hue = if max == r {
        60.0 * (((g - b) / (max - min)).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / (max - min) + 2.0)
    } else {
        60.0 * ((r - g) / (max - min) + 4.0)
    };
    Some(match hue {
        h if !(15.0..345.0).contains(&h) => "red",
        h if h < 45.0 => {
            if l < 0.4 {
                "brown"
            } else {
                "orange"
            }
        }
        h if h < 70.0 => "yellow",
        h if h < 160.0 => "green",
        h if h < 195.0 => "teal",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    })
}

/// "portrait", "landscape", "square" or "panorama".
pub fn shape_tag(width: u32, height: u32) -> &'static str {
    let a = width as f32 / height.max(1) as f32;
    if a >= 2.2 {
        "panorama"
    } else if a > 1.1 {
        "landscape"
    } else if a < 0.9 {
        "portrait"
    } else {
        "square"
    }
}

/// "short" under 10 s, "long" from 60 s, nothing in between.
pub fn length_tag(duration_ms: u64) -> Option<&'static str> {
    match duration_ms {
        0..10_000 => Some("short"),
        60_000.. => Some("long"),
        _ => None,
    }
}

fn local(tag: &str, source: &str) -> AutoTag {
    AutoTag { tag: tag.to_owned(), source: format!("local:{source}"), confidence: None }
}

/// Every local auto-tag for a picture or video, given its main colours.
pub fn local_tags(colors: &[String], width: u32, height: u32, duration_ms: Option<u64>) -> Vec<AutoTag> {
    let mut out = Vec::new();
    for c in colors.iter().take(2) {
        if let Some(name) = color_name(c) {
            if !out.iter().any(|t: &AutoTag| t.tag == name) {
                out.push(local(name, "color"));
            }
        }
    }
    out.push(local(shape_tag(width, height), "shape"));
    if let Some(t) = duration_ms.and_then(length_tag) {
        out.push(local(t, "length"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    #[test]
    fn main_colours_most_common_first() {
        // Top 70% blue, bottom 30% orange.
        let (w, h) = (100, 100);
        let mut px = solid([30, 80, 220], w, h);
        for p in (70 * w as usize)..(w * h) as usize {
            px[p * 4..p * 4 + 3].copy_from_slice(&[240, 140, 40]);
        }
        let c = dominant_colors(&px, w, h, 3);
        assert_eq!(c.len(), 2);
        assert_eq!(color_name(&c[0]), Some("blue"));
        assert_eq!(color_name(&c[1]), Some("orange"));
    }

    #[test]
    fn names_and_shapes() {
        assert_eq!(color_name("#e5534b"), Some("red"));
        assert_eq!(color_name("#57ab5a"), Some("green"));
        assert_eq!(color_name("#101010"), Some("black"));
        assert_eq!(color_name("#f8f8f8"), Some("white"));
        assert_eq!(color_name("#808585"), Some("grey"));
        assert_eq!(color_name("#6b3f1d"), Some("brown"));
        assert_eq!(shape_tag(1920, 1080), "landscape");
        assert_eq!(shape_tag(1080, 1920), "portrait");
        assert_eq!(shape_tag(500, 500), "square");
        assert_eq!(shape_tag(3000, 1000), "panorama");
        assert_eq!(length_tag(4_000), Some("short"));
        assert_eq!(length_tag(30_000), None);
        let tags = local_tags(&["#1e50dc".into(), "#2050e0".into()], 1920, 1080, Some(4_000));
        let names: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
        assert_eq!(names, ["blue", "landscape", "short"]);
        assert!(tags.iter().all(|t| t.source.starts_with("local:")));
    }
}
