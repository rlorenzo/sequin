//! Perceptual hashing with border-trim preprocessing.
//!
//! Implements classic pHash directly (grayscale → 64×64 → 2D DCT-II →
//! top-left 16×16 coefficients → bit = coefficient > median), matching the
//! construction of Python `imagehash.phash(img, hash_size=16)` that was
//! validated on real studio deliveries. We deliberately do NOT use
//! `image_hasher`'s `preproc_dct()` (32×32 DCT + mean threshold): on real
//! portrait batches it collapsed 51 of 62 photos into one cluster because
//! studio portraits share too much coarse structure.
//!
//! Two hashes per photo: one of the full frame, one after trimming
//! uniform-color borders. Matching takes the minimum distance of the two,
//! which lets a bordered/framed variant match its borderless original.

use crate::Photo;
use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{GenericImageView, GrayImage, RgbImage};
use rayon::prelude::*;
use rustdct::DctPlanner;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Hash grid: 16×16 low-frequency DCT coefficients = 256 bits.
pub const HASH_BITS_SIDE: usize = 16;
/// DCT input size = hash side × 4, matching imagehash's `highfreq_factor=4`.
const DCT_SIZE: usize = HASH_BITS_SIDE * 4;

/// Images are downscaled to this bound before border-trim analysis; plenty
/// for a 256-bit hash and much faster than working on 3000px originals.
const WORK_SIZE: u32 = 800;

/// Per-channel tolerance when deciding a row/column is "uniform" border.
const BORDER_TOL: i32 = 18;

/// 256-bit perceptual hash, stored as 4 little-endian u64 words, hex-encoded
/// in [`Photo`] for serialization.
pub type Hash256 = [u64; 4];

pub fn hash_to_hex(h: &Hash256) -> String {
    h.iter().map(|w| format!("{w:016x}")).collect()
}

pub fn hash_from_hex(s: &str) -> Result<Hash256> {
    anyhow::ensure!(s.len() == 64, "hash hex must be 64 chars, got {}", s.len());
    let mut out = [0u64; 4];
    for (i, chunk) in s.as_bytes().chunks(16).enumerate() {
        out[i] = u64::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
    }
    Ok(out)
}

pub fn hamming(a: &Hash256, b: &Hash256) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// Hamming distance between two photos, taking the minimum over full-frame
/// and border-cropped hashes.
pub fn distance(a: &Photo, b: &Photo) -> Result<u32> {
    let d_full = hamming(&hash_from_hex(&a.hash_full)?, &hash_from_hex(&b.hash_full)?);
    let d_crop = hamming(
        &hash_from_hex(&a.hash_cropped)?,
        &hash_from_hex(&b.hash_cropped)?,
    );
    Ok(d_full.min(d_crop))
}

/// File extensions (lowercased) Sequin treats as photos.
pub const PHOTO_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

/// List the photo files in `dir` (non-recursive), sorted by name.
pub fn list_photo_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()).map(str::to_lowercase),
                Some(ref e) if PHOTO_EXTENSIONS.contains(&e.as_str())
            )
        })
        .collect();
    paths.sort();
    Ok(paths)
}

/// Hash every JPEG/PNG in `dir` in parallel. Non-image files are skipped.
pub fn scan_dir(dir: &Path) -> Result<Vec<Photo>> {
    list_photo_paths(dir)?
        .par_iter()
        .map(|p| hash_photo(p))
        .collect::<Result<Vec<_>>>()
}

pub fn hash_photo(path: &Path) -> Result<Photo> {
    Ok(hash_photo_with_work(path)?.0)
}

/// Like [`hash_photo`], but also returns the ~[`WORK_SIZE`]px working image so
/// callers (thumbnail generation) can reuse the single decode. The hash
/// computation path is identical to [`hash_photo`]'s.
pub fn hash_photo_with_work(path: &Path) -> Result<(Photo, RgbImage)> {
    let img = image::open(path).with_context(|| format!("opening {}", path.display()))?;
    let (ow, oh) = img.dimensions();
    let img = img.resize(WORK_SIZE, WORK_SIZE, FilterType::Triangle);
    let rgb = img.to_rgb8();

    let hash_full = phash(&to_dct_input(&rgb));

    let (cropped, border_fraction) = trim_border(&rgb);
    let hash_cropped = if border_fraction > 0.005 {
        phash(&to_dct_input(&cropped))
    } else {
        hash_full
    };

    let photo = Photo {
        path: path.to_path_buf(),
        hash_full: hash_to_hex(&hash_full),
        hash_cropped: hash_to_hex(&hash_cropped),
        border_fraction,
        width: ow,
        height: oh,
    };
    Ok((photo, rgb))
}

/// Grayscale + resize to the square DCT input (aspect is intentionally
/// discarded, as in imagehash).
fn to_dct_input(rgb: &RgbImage) -> GrayImage {
    let gray = image::imageops::grayscale(rgb);
    image::imageops::resize(
        &gray,
        DCT_SIZE as u32,
        DCT_SIZE as u32,
        FilterType::Lanczos3,
    )
}

/// The planned 64-point DCT-II, shared process-wide: planning is not free and
/// `phash` runs twice per photo across the rayon pool. rustdct transforms are
/// `Send + Sync` and take `&self`, so one plan serves every thread.
fn dct64() -> &'static Arc<dyn rustdct::TransformType2And3<f32>> {
    static DCT: OnceLock<Arc<dyn rustdct::TransformType2And3<f32>>> = OnceLock::new();
    DCT.get_or_init(|| DctPlanner::new().plan_dct2(DCT_SIZE))
}

/// Classic pHash: 2D DCT-II, keep top-left 16×16 (lowest frequencies,
/// including DC), threshold at the median.
fn phash(gray: &GrayImage) -> Hash256 {
    let n = DCT_SIZE;
    let mut data: Vec<f32> = gray.pixels().map(|p| p[0] as f32).collect();

    let dct = dct64();
    let mut scratch = vec![0.0f32; dct.get_scratch_len()];

    // rows
    for row in data.chunks_exact_mut(n) {
        dct.process_dct2_with_scratch(row, &mut scratch);
    }
    // columns
    let mut col = vec![0.0f32; n];
    for c in 0..n {
        for r in 0..n {
            col[r] = data[r * n + c];
        }
        dct.process_dct2_with_scratch(&mut col, &mut scratch);
        for r in 0..n {
            data[r * n + c] = col[r];
        }
    }

    // top-left 16×16 block
    let mut low = [0.0f32; HASH_BITS_SIDE * HASH_BITS_SIDE];
    for r in 0..HASH_BITS_SIDE {
        for c in 0..HASH_BITS_SIDE {
            low[r * HASH_BITS_SIDE + c] = data[r * n + c];
        }
    }
    let mut sorted = low;
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.len() / 2;
    let median = (sorted[mid - 1] + sorted[mid]) / 2.0;

    let mut hash = [0u64; 4];
    for (i, v) in low.iter().enumerate() {
        if *v > median {
            hash[i / 64] |= 1 << (i % 64);
        }
    }
    hash
}

/// Trim uniform-color margins (up to 1/3 from each side). Returns the cropped
/// image and the fraction of area removed.
fn trim_border(img: &RgbImage) -> (RgbImage, f32) {
    let (w, h) = img.dimensions();
    let is_uniform_row = |y: u32| {
        let base = img.get_pixel(0, y);
        (0..w).step_by((w as usize / 64).max(1)).all(|x| {
            let p = img.get_pixel(x, y);
            (0..3)
                .map(|c| (p[c] as i32 - base[c] as i32).abs())
                .sum::<i32>()
                < BORDER_TOL * 3
        })
    };
    let is_uniform_col = |x: u32| {
        let base = img.get_pixel(x, 0);
        (0..h).step_by((h as usize / 64).max(1)).all(|y| {
            let p = img.get_pixel(x, y);
            (0..3)
                .map(|c| (p[c] as i32 - base[c] as i32).abs())
                .sum::<i32>()
                < BORDER_TOL * 3
        })
    };

    let mut top = 0;
    while top < h / 3 && is_uniform_row(top) {
        top += 1;
    }
    let mut bottom = h - 1;
    while bottom > 2 * h / 3 && is_uniform_row(bottom) {
        bottom -= 1;
    }
    let mut left = 0;
    while left < w / 3 && is_uniform_col(left) {
        left += 1;
    }
    let mut right = w - 1;
    while right > 2 * w / 3 && is_uniform_col(right) {
        right -= 1;
    }

    let cw = right - left + 1;
    let ch = bottom - top + 1;
    let frac = 1.0 - (cw as f32 * ch as f32) / (w as f32 * h as f32);
    let cropped = image::imageops::crop_imm(img, left, top, cw, ch).to_image();
    (cropped, frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_hamming() {
        let h: Hash256 = [0x0123_4567_89ab_cdef, 0, u64::MAX, 42];
        let hex = hash_to_hex(&h);
        assert_eq!(hex.len(), 64);
        assert_eq!(hash_from_hex(&hex).unwrap(), h);

        assert_eq!(hamming(&h, &h), 0);
        let mut flipped = h;
        flipped[0] ^= 0b101; // two bits
        flipped[3] ^= 1 << 63; // one bit, in another word
        assert_eq!(hamming(&h, &flipped), 3);
    }

    #[test]
    fn hash_from_hex_rejects_bad_input() {
        assert!(hash_from_hex("abc").is_err(), "wrong length");
        assert!(hash_from_hex(&"g".repeat(64)).is_err(), "non-hex chars");
    }
}
