use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use wall_format::OriginalInfo;

use crate::{Limits, MediaError};

/// An image ready to store with `Library::import_bytes`.
#[derive(Debug, Clone)]
pub struct PreparedImage {
    pub bytes: Vec<u8>,
    /// Extension to store under: `jpg`, `png` or `webp`.
    pub ext: &'static str,
    pub width: u32,
    pub height: u32,
    /// Set when the image was scaled down; describes the file before.
    pub original: Option<OriginalInfo>,
}

impl PreparedImage {
    pub fn was_resized(&self) -> bool {
        self.original.is_some()
    }
}

const JPEG_QUALITY: u8 = 90;

/// Applies the width rule. Images within the limit keep their exact bytes.
/// Wider ones are resized (Lanczos3, aspect kept) and re-encoded: JPEG stays
/// JPEG, PNG stays PNG, WebP becomes PNG if it has transparency, else JPEG.
/// GIFs are refused here: they go through the video path so they can be
/// frame-stepped.
pub fn prepare_image(bytes: &[u8], limits: &Limits) -> Result<PreparedImage, MediaError> {
    let format = image::guess_format(bytes)?;
    let ext = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        other => return Err(MediaError::Unsupported(format!("{other:?}"))),
    };
    let img = image::load_from_memory_with_format(bytes, format)?;
    let (width, height) = img.dimensions();

    if width <= limits.max_image_width {
        return Ok(PreparedImage { bytes: bytes.to_vec(), ext, width, height, original: None });
    }

    let new_w = limits.max_image_width;
    let new_h = ((height as f64) * (new_w as f64) / (width as f64)).round().max(1.0) as u32;
    let resized = img.resize_exact(new_w, new_h, FilterType::Lanczos3);
    let has_alpha = resized.color().has_alpha();

    let (out, out_ext) = match format {
        ImageFormat::Png => (encode_png(&resized)?, "png"),
        ImageFormat::WebP if has_alpha => (encode_png(&resized)?, "png"),
        _ => (encode_jpeg(&resized)?, "jpg"),
    };
    Ok(PreparedImage {
        bytes: out,
        ext: out_ext,
        width: new_w,
        height: new_h,
        original: Some(OriginalInfo { width, height, bytes: bytes.len() as u64, kept_as: None }),
    })
}

fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, MediaError> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn encode_jpeg(img: &DynamicImage) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    let rgb = img.to_rgb8();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&rgb)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        encode_jpeg(&DynamicImage::ImageRgb8(img)).unwrap()
    }

    fn png_rgba(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, Rgba([255, 0, 0, 100]));
        encode_png(&DynamicImage::ImageRgba8(img)).unwrap()
    }

    #[test]
    fn small_image_is_untouched() {
        let src = jpeg(2048, 300);
        let out = prepare_image(&src, &Limits::default()).unwrap();
        assert_eq!(out.bytes, src);
        assert_eq!((out.width, out.height, out.ext), (2048, 300, "jpg"));
        assert!(!out.was_resized());
    }

    #[test]
    fn wide_jpeg_is_scaled_to_2048_keeping_aspect() {
        let src = jpeg(4000, 3000);
        let out = prepare_image(&src, &Limits::default()).unwrap();
        assert_eq!((out.width, out.height, out.ext), (2048, 1536, "jpg"));
        let back = image::load_from_memory(&out.bytes).unwrap();
        assert_eq!(back.dimensions(), (2048, 1536));
        let orig = out.original.unwrap();
        assert_eq!((orig.width, orig.height, orig.bytes), (4000, 3000, src.len() as u64));
    }

    #[test]
    fn wide_png_keeps_alpha() {
        let out = prepare_image(&png_rgba(3000, 10), &Limits::default()).unwrap();
        assert_eq!((out.width, out.height, out.ext), (2048, 7, "png"));
        let back = image::load_from_memory(&out.bytes).unwrap();
        assert!(back.color().has_alpha());
    }

    #[test]
    fn custom_limit_applies() {
        let limits = Limits { max_image_width: 100, ..Limits::default() };
        let out = prepare_image(&jpeg(300, 300), &limits).unwrap();
        assert_eq!((out.width, out.height), (100, 100));
    }

    #[test]
    fn gif_and_garbage_are_refused() {
        let mut gif = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::new(4, 4)).write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif).unwrap();
        assert!(matches!(prepare_image(&gif, &Limits::default()), Err(MediaError::Unsupported(_))));
        assert!(prepare_image(b"not an image", &Limits::default()).is_err());
    }
}
