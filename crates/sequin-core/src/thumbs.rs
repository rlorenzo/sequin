//! Thumbnail generation fused with hashing: one decode per photo.
//!
//! [`scan_dir_with_thumbs`] is the GUI-facing sibling of
//! [`hashing::scan_dir`]: it produces the same [`Photo`] hashes (via the same
//! code path) while also writing a display thumbnail from the already-decoded
//! working image, computing a black-&-white flag, and tolerating unreadable
//! files (skip and report, never fail the whole scan).

use crate::{hashing, Photo};
use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::RgbImage;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Thumbnails are bounded to this dimension: sharp at 176pt @2x retina.
pub const THUMB_MAX_DIM: u32 = 512;
const JPEG_QUALITY: u8 = 82;

/// Mean per-pixel channel spread (max − min, 0–255) below which a photo is
/// considered black & white. B&W conversions measure ≈0; the margin absorbs
/// JPEG chroma noise.
const BW_SATURATION_THRESHOLD: f32 = 8.0;

/// One successfully scanned photo.
pub struct Scanned {
    pub photo: Photo,
    /// Path of the written (or reused cached) thumbnail JPEG.
    pub thumb: PathBuf,
    /// True when the photo is (near-)monochrome, e.g. a B&W conversion.
    pub is_bw: bool,
}

/// Result of scanning a folder: photos that hashed, files that didn't.
pub struct ScanReport {
    pub photos: Vec<Scanned>,
    pub failures: Vec<(PathBuf, String)>,
}

/// Thumbnail cache directory for one source folder:
/// `<cache_root>/<hash-of-canonical-source-path>/`. FNV-1a keeps the key
/// stable across Rust releases (std's `DefaultHasher` is not guaranteed to
/// be), so a toolchain upgrade never invalidates the cache. A collision just
/// costs a regeneration.
pub fn cache_dir_for(cache_root: &Path, source_dir: &Path) -> PathBuf {
    let canon = source_dir
        .canonicalize()
        .unwrap_or_else(|_| source_dir.to_path_buf());
    let key = canon
        .to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325u64, |acc, b| {
            (acc ^ b as u64).wrapping_mul(0x100000001b3)
        });
    cache_root.join(format!("{key:016x}"))
}

/// Thumbnail file for a photo: `<cache_dir>/<source file name>.jpg`. Keeping
/// the full source name (extension included) avoids stem collisions like
/// `a.jpg` vs `a.png`.
pub fn thumb_path(cache_dir: &Path, photo_path: &Path) -> PathBuf {
    let name = photo_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("photo");
    cache_dir.join(format!("{name}.jpg"))
}

/// Hash + thumbnail every JPEG/PNG in `dir` in parallel. `progress(done,
/// total)` is called from worker threads after each file, success or failure.
pub fn scan_dir_with_thumbs(
    dir: &Path,
    cache_dir: &Path,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<ScanReport> {
    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("creating thumbnail cache {}", cache_dir.display()))?;

    let paths = hashing::list_photo_paths(dir)?;
    let total = paths.len();
    let done = AtomicUsize::new(0);
    let results: Vec<(PathBuf, Result<Scanned>)> = paths
        .par_iter()
        .map(|p| {
            let r = scan_one(p, cache_dir);
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            (p.clone(), r)
        })
        .collect();

    let mut photos = Vec::new();
    let mut failures = Vec::new();
    for (path, result) in results {
        match result {
            Ok(s) => photos.push(s),
            Err(e) => failures.push((path, format!("{e:#}"))),
        }
    }
    Ok(ScanReport { photos, failures })
}

fn scan_one(path: &Path, cache_dir: &Path) -> Result<Scanned> {
    let (photo, work) = hashing::hash_photo_with_work(path)?;
    let is_bw = mean_saturation(&work) < BW_SATURATION_THRESHOLD;
    let thumb = thumb_path(cache_dir, path);
    if !thumb_is_fresh(&thumb, path) {
        write_thumb(&work, &thumb)
            .with_context(|| format!("writing thumbnail for {}", path.display()))?;
    }
    Ok(Scanned {
        photo,
        thumb,
        is_bw,
    })
}

/// A cached thumbnail is reusable when it exists and is at least as new as
/// its source file.
fn thumb_is_fresh(thumb: &Path, source: &Path) -> bool {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    matches!((mtime(thumb), mtime(source)), (Some(t), Some(s)) if t >= s)
}

fn write_thumb(work: &RgbImage, dest: &Path) -> Result<()> {
    let (w, h) = work.dimensions();
    let img = image::DynamicImage::ImageRgb8(work.clone());
    let img = if w.max(h) > THUMB_MAX_DIM {
        img.resize(THUMB_MAX_DIM, THUMB_MAX_DIM, FilterType::Lanczos3)
    } else {
        img
    };
    // Encode to a same-directory temp file and rename into place, so an
    // interrupted write can never leave a partial JPEG that `thumb_is_fresh`
    // would treat as valid (same pattern as the EXIF writes in `exif.rs`).
    let tmp = dest.with_extension(format!("tmp{}", std::process::id()));
    let write = (|| -> Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
        img.write_with_encoder(enc)?;
        use std::io::Write;
        out.flush()?;
        Ok(())
    })();
    match write {
        Ok(()) => Ok(std::fs::rename(&tmp, dest)?),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Mean channel spread over a sparse pixel grid; 0 for true grayscale.
fn mean_saturation(img: &RgbImage) -> f32 {
    let (w, h) = img.dimensions();
    let step_x = (w / 64).max(1);
    let step_y = (h / 64).max(1);
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in (0..h).step_by(step_y as usize) {
        for x in (0..w).step_by(step_x as usize) {
            let p = img.get_pixel(x, y);
            let max = p.0.iter().max().unwrap();
            let min = p.0.iter().min().unwrap();
            sum += (max - min) as u64;
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    sum as f32 / n as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn save_scene(dir: &Path, name: &str, gray: bool) -> PathBuf {
        // Low-frequency gradient scene (pHash needs low-freq structure).
        let mut img = RgbImage::new(640, 480);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = ((x as f32 / 640.0) * 180.0 + (y as f32 / 480.0) * 60.0) as u8;
            *p = if gray {
                Rgb([v, v, v])
            } else {
                Rgb([v, (v / 2).saturating_add(40), 200u8.saturating_sub(v / 2)])
            };
        }
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn scan_produces_thumbs_flags_and_tolerates_bad_files() {
        let src = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        save_scene(src.path(), "color.jpg", false);
        save_scene(src.path(), "mono.jpg", true);
        std::fs::write(src.path().join("broken.jpg"), b"not an image").unwrap();

        let report = scan_dir_with_thumbs(src.path(), cache.path(), &|_, _| {}).unwrap();

        assert_eq!(report.photos.len(), 2);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].0.ends_with("broken.jpg"));

        for s in &report.photos {
            assert!(s.thumb.exists(), "thumbnail missing for {:?}", s.photo.path);
            let t = image::open(&s.thumb).unwrap();
            assert!(t.width() <= THUMB_MAX_DIM && t.height() <= THUMB_MAX_DIM);
            // 640×480 source → aspect preserved within rounding.
            let ratio = t.width() as f32 / t.height() as f32;
            assert!(
                (ratio - 640.0 / 480.0).abs() < 0.05,
                "aspect drifted: {ratio}"
            );
        }

        let bw_flags: Vec<bool> = report
            .photos
            .iter()
            .map(|s| (s.photo.path.file_name().unwrap() == "mono.jpg", s.is_bw))
            .map(|(is_mono, is_bw)| is_mono == is_bw)
            .collect();
        assert!(bw_flags.iter().all(|ok| *ok), "b&w flags wrong");
    }

    #[test]
    fn fresh_thumbnails_are_not_rewritten() {
        let src = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let photo = save_scene(src.path(), "one.jpg", false);

        scan_dir_with_thumbs(src.path(), cache.path(), &|_, _| {}).unwrap();
        let thumb = thumb_path(cache.path(), &photo);

        // Overwrite the (fresh) thumbnail with a sentinel; a rescan must reuse
        // it rather than regenerate.
        std::fs::write(&thumb, b"sentinel").unwrap();
        scan_dir_with_thumbs(src.path(), cache.path(), &|_, _| {}).unwrap();
        assert_eq!(std::fs::read(&thumb).unwrap(), b"sentinel");
    }

    #[test]
    fn hashes_match_the_plain_hashing_path() {
        let src = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = save_scene(src.path(), "same.jpg", false);

        let report = scan_dir_with_thumbs(src.path(), cache.path(), &|_, _| {}).unwrap();
        let via_thumbs = &report.photos[0].photo;
        let via_hashing = hashing::hash_photo(&path).unwrap();
        assert_eq!(via_thumbs.hash_full, via_hashing.hash_full);
        assert_eq!(via_thumbs.hash_cropped, via_hashing.hash_cropped);
    }
}
