// SPDX-License-Identifier: GPL-3.0-or-later
//! Pictures on the clipboard: the OS hands out raw RGBA, the wire carries PNG.

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageFormat, ImageReader, Limits};

use crate::ClipboardError;

/// The widest or tallest picture taken from a peer. 16384 covers an 8K screen
/// twice over.
const MAX_SIDE: u32 = 16_384;
/// The most pixels taken from a peer (about 64 megapixels, 256 MiB as RGBA).
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// Raw pixels: `width * height * 4` bytes of RGBA.
#[derive(Debug)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// PNG with fast compression: a screenshot is sent once and soon replaced, so
/// speed matters more than the last few percent of size.
pub fn to_png(rgba: &Rgba) -> Result<Vec<u8>, ClipboardError> {
    let expected = rgba.width as u64 * rgba.height as u64 * 4;
    if rgba.bytes.len() as u64 != expected {
        return Err(ClipboardError(format!(
            "picture of {}x{} has {} bytes, expected {expected}",
            rgba.width,
            rgba.height,
            rgba.bytes.len()
        )));
    }
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            &rgba.bytes,
            rgba.width,
            rgba.height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| ClipboardError(e.to_string()))?;
    Ok(out)
}

/// The pixels of a PNG a peer sent. Its size is checked from the header before
/// anything is decoded, so a small file cannot claim a huge picture.
pub fn from_png(png: &[u8]) -> Result<Rgba, ClipboardError> {
    let bad = |e: image::ImageError| ClipboardError(format!("picture: {e}"));
    let (width, height) = ImageReader::with_format(Cursor::new(png), ImageFormat::Png)
        .into_dimensions()
        .map_err(bad)?;
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || width as u64 * height as u64 > MAX_PIXELS
    {
        return Err(ClipboardError(format!(
            "picture of {width}x{height} is too large to take"
        )));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_PIXELS * 4 + 64 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
    reader.limits(limits);
    let decoded = reader.decode().map_err(bad)?.into_rgba8();
    Ok(Rgba {
        width,
        height,
        bytes: decoded.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Rgba {
        let mut bytes = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let on = (x + y) % 2 == 0;
                // Half-transparent squares: alpha must survive the trip.
                bytes.extend_from_slice(if on {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 128]
                });
            }
        }
        Rgba {
            width: w,
            height: h,
            bytes,
        }
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn pixels_survive_png_including_alpha() {
        let src = checker(37, 21);
        let png = to_png(&src).unwrap();
        let back = from_png(&png).unwrap();
        assert_eq!((back.width, back.height), (37, 21));
        assert_eq!(back.bytes, src.bytes);
    }

    #[test]
    fn a_wrong_byte_count_is_refused() {
        let mut src = checker(4, 4);
        src.bytes.pop();
        assert!(to_png(&src).is_err());
    }

    #[test]
    fn something_that_is_not_a_png_is_refused() {
        assert!(from_png(b"not a picture at all").is_err());
    }

    #[test]
    fn a_header_claiming_a_huge_picture_is_refused_before_decoding() {
        let mut png = to_png(&checker(2, 2)).unwrap();
        // IHDR width and height sit at bytes 16..24; claim 60000 x 60000.
        png[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        // A valid checksum, so it is the size check that refuses it, not the CRC.
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let err = from_png(&png).unwrap_err();
        assert!(err.0.contains("too large"), "{err}");
    }
}
