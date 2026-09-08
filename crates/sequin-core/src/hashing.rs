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
use image::{DynamicImage, GenericImageView, GrayImage, RgbImage};
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

/// The scaled-decode request is expressed in `u16`. Keep `WORK_SIZE` inside
/// that range so a future bump fails the build rather than silently disabling
/// the fast path and quietly restoring full-size decodes.
const _: () = assert!(WORK_SIZE <= u16::MAX as u32);

/// Per-channel tolerance when deciding a row/column is "uniform" border.
const BORDER_TOL: i32 = 18;

/// Default ceiling on concurrent decodes during a scan.
///
/// Decoding dominates scan memory: a 24MP studio JPEG expands to ~72 MB of
/// RGB, and that full-size buffer is still alive while it is downscaled to
/// [`WORK_SIZE`]. The global rayon pool runs one decode per core, so a
/// 10-core machine peaks near 750 MB and is the first thing the OS reaps
/// under memory pressure. Capping the decoders drops peak RSS roughly
/// proportionally; the rest of the per-photo work (DCT, border trim) is
/// cheap and already bounded, so throughput barely moves.
const DEFAULT_MAX_DECODES: usize = 4;

/// Ceiling on the memory a single scaled decode may commit, mirroring the
/// 512 MiB allocation limit `image::open` applies by default — the guard the
/// JPEG fast path would otherwise skip.
///
/// [`decode_jpeg_scaled`] hands bytes straight to `jpeg_decoder`, and
/// `scale()` shrinks the IDCT *output* only: a progressive frame still
/// allocates one `i16` coefficient per full-resolution sample the moment the
/// decoder reaches the first scan. A header advertising huge dimensions can
/// therefore commit gigabytes before `decode` returns an error, which no
/// `Result` can recover from. Over-budget files fall back to `image::open`,
/// which checks the same limit before it allocates anything.
const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;

/// Worker count for a directory scan. `SEQUIN_SCAN_THREADS` overrides it
/// (useful on a memory-starved machine, or to spend more RAM for speed);
/// an unset, unparsable, or zero value falls back to the default.
pub(crate) fn scan_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    resolve_scan_threads(std::env::var("SEQUIN_SCAN_THREADS").ok().as_deref(), cores)
}

/// The override-vs-default decision, split from the environment read so it is
/// testable without mutating process-global state. `cores` is assumed >= 1.
fn resolve_scan_threads(override_var: Option<&str>, cores: usize) -> usize {
    override_var
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| cores.min(DEFAULT_MAX_DECODES))
}

/// Run `f` on a rayon pool bounded to [`scan_threads`] workers, so scans do
/// not inherit the global pool's one-task-per-core width.
pub(crate) fn with_scan_pool<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(scan_threads())
        .build()
        .context("building scan thread pool")?;
    Ok(pool.install(f))
}

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

/// The subset of [`PHOTO_EXTENSIONS`] carrying a DCT the decoder can scale
/// down; see [`decode_jpeg_scaled`].
const JPEG_EXTENSIONS: [&str; 2] = ["jpg", "jpeg"];

/// Case-insensitive extension test — the single place file types are matched,
/// so the scan filter and the JPEG fast path cannot drift apart.
fn has_extension(path: &Path, allowed: &[&str]) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| allowed.iter().any(|a| ext.eq_ignore_ascii_case(a)))
}

/// List the photo files in `dir` (non-recursive), sorted by name.
pub fn list_photo_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| has_extension(p, &PHOTO_EXTENSIONS))
        .collect();
    paths.sort();
    Ok(paths)
}

/// Hash every JPEG/PNG in `dir` in parallel. Non-image files are skipped.
pub fn scan_dir(dir: &Path) -> Result<Vec<Photo>> {
    let paths = list_photo_paths(dir)?;
    with_scan_pool(|| {
        paths
            .par_iter()
            .map(|p| hash_photo(p))
            .collect::<Result<Vec<_>>>()
    })?
}

pub fn hash_photo(path: &Path) -> Result<Photo> {
    Ok(hash_photo_with_work(path)?.0)
}

/// Like [`hash_photo`], but also returns the ~[`WORK_SIZE`]px working image so
/// callers (thumbnail generation) can reuse the single decode. The hash
/// computation path is identical to [`hash_photo`]'s.
pub fn hash_photo_with_work(path: &Path) -> Result<(Photo, RgbImage)> {
    let (img, (ow, oh)) = decode_at_work_scale(path)?;
    let rgb = to_work_image(img);
    let (hash_full, hash_cropped, border_fraction) = hashes_of(&rgb);

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

/// Downscale a decoded image to the [`WORK_SIZE`] working copy that hashing,
/// border-trimming and thumbnailing all share.
fn to_work_image(img: DynamicImage) -> RgbImage {
    img.resize(WORK_SIZE, WORK_SIZE, FilterType::Triangle)
        .to_rgb8()
}

/// Both hashes of a working image (full frame, border-trimmed) plus the
/// fraction of area the trim removed. Split out so tests can hash an image
/// they decoded themselves without going through the file-path entry point.
fn hashes_of(rgb: &RgbImage) -> (Hash256, Hash256, f32) {
    let hash_full = phash(&to_dct_input(rgb));
    let (cropped, border_fraction) = trim_border(rgb);
    let hash_cropped = if border_fraction > 0.005 {
        phash(&to_dct_input(&cropped))
    } else {
        hash_full
    };
    (hash_full, hash_cropped, border_fraction)
}

/// Decode `path` large enough to build the [`WORK_SIZE`] working image, and
/// report the ORIGINAL pixel dimensions alongside it.
///
/// JPEG stores 8x8 blocks of frequency coefficients, so a decoder can run a
/// reduced inverse DCT and emit the image at 1/8, 1/4 or 1/2 scale without
/// ever materializing the full-resolution pixels. Sequin only ever looks at a
/// `WORK_SIZE` copy, so a 24MP original decodes at 1/4 into ~4.5 MB instead
/// of ~72 MB -- and the skipped IDCT work makes it faster too.
///
/// `image::open` (zune-jpeg in image 0.25) exposes no scaling API, so JPEGs
/// go through `jpeg_decoder` directly. PNGs, CMYK and 16-bit greyscale JPEGs,
/// and anything the fast path chokes on fall back to the full decode, which
/// stays the reference behaviour.
///
/// The fast path is ON by default. A reduced IDCT is a different
/// reconstruction, so it shifts hash bits (measured: max 12 of 256 on
/// synthetic 24MP scenes, grouping unchanged) -- well inside the real-photo
/// margin behind invariant 4, but NOT yet confirmed by the golden test,
/// which needs the real delivery. Set `SEQUIN_FULL_DECODE=1` to force every
/// file down the original path: that is how the golden test A/Bs the two,
/// and it is the rollback if the scaled run ever fails to reproduce all 34
/// groups.
///
/// The returned dimensions are the source file's, NOT the decoded buffer's:
/// [`Photo::width`]/[`height`] describe the photo on disk and must not change
/// just because the decode was scaled.
fn decode_at_work_scale(path: &Path) -> Result<(DynamicImage, (u32, u32))> {
    if scaled_decode_enabled() {
        if let Some(pair) = decode_jpeg_scaled(path) {
            return Ok(pair);
        }
    }
    let img = image::open(path).with_context(|| format!("opening {}", path.display()))?;
    let dims = img.dimensions();
    Ok((img, dims))
}

/// True unless `SEQUIN_FULL_DECODE` is set to anything but "0". Resolved once
/// per process (like [`dct64`]) so a single scan can never decode some photos
/// scaled and others full.
fn scaled_decode_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| !std::env::var("SEQUIN_FULL_DECODE").is_ok_and(|v| v != "0"))
}

/// The scaled-JPEG fast path. Returns `None` -- never an error -- for any file
/// it cannot handle, so the caller simply falls back to the full decode and a
/// genuinely broken file still surfaces its real error from there.
///
/// The unwind guard is part of that contract. `jpeg_decoder` panics rather
/// than erroring on a few malformed frame headers: `info()` matches only
/// component counts 1, 3 and 4, and a baseline frame declaring 2 or 5+
/// components parses cleanly before reaching it. `image::open` never exposed
/// those paths, so catching here keeps one bad file a per-file failure
/// instead of unwinding the whole scan out of the rayon pool.
fn decode_jpeg_scaled(path: &Path) -> Option<(DynamicImage, (u32, u32))> {
    if !has_extension(path, &JPEG_EXTENSIONS) {
        return None;
    }
    std::panic::catch_unwind(|| decode_jpeg_scaled_inner(path))
        .ok()
        .flatten()
}

/// Worst case bytes `jpeg_decoder` commits for a frame of this size: one
/// `i16` coefficient per sample, no chroma subsampling, rounded up to whole
/// 8x8 blocks. Dimensions are `u16` and components at most 4, so the product
/// tops out around 34 GB and cannot overflow `u64`.
fn decode_budget_bytes(width: u16, height: u16, components: u64) -> u64 {
    let blocks = |n: u16| u64::from(n).div_ceil(8);
    blocks(width) * blocks(height) * 64 * 2 * components
}

/// The body of [`decode_jpeg_scaled`], split out so the unwind guard wraps
/// every decoder call rather than just the first.
fn decode_jpeg_scaled_inner(path: &Path) -> Option<(DynamicImage, (u32, u32))> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    // Bounds the decoded output plane. The coefficient budget below is the
    // load-bearing half: this limit is only consulted once the scans are
    // already decoded, so it cannot catch the earlier progressive allocation.
    decoder.set_max_decoding_buffer_size(usize::try_from(MAX_DECODE_BYTES).unwrap_or(usize::MAX));
    decoder.read_info().ok()?;
    // Full size first: it is what `Photo` reports, and `scale` overwrites it.
    let full = decoder.info()?;
    let (ow, oh) = (u32::from(full.width), u32::from(full.height));

    // Refuse an over-budget frame BEFORE `decode` allocates for it. Real
    // deliveries are nowhere near the limit (a 24MP RGB frame budgets
    // ~144 MB); anything that is falls back to the guarded `image::open`.
    let components = match full.pixel_format {
        jpeg_decoder::PixelFormat::L8 | jpeg_decoder::PixelFormat::L16 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::CMYK32 => 4,
    };
    if decode_budget_bytes(full.width, full.height, components) > MAX_DECODE_BYTES {
        return None;
    }

    // Ask for a WORK_SIZE box. `choose_idct_size` picks the smallest scale
    // whose output still covers it on at least one axis, so the long edge is
    // always >= WORK_SIZE and the later resize never has to upscale.
    // Infallible given the const assert above; `?` keeps this function
    // panic-free like every other step in it.
    let side = u16::try_from(WORK_SIZE).ok()?;
    decoder.scale(side, side).ok()?;

    let info = decoder.info()?;
    let pixels = decoder.decode().ok()?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));

    let img = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, pixels)?)
        }
        jpeg_decoder::PixelFormat::L8 => {
            DynamicImage::ImageLuma8(GrayImage::from_raw(w, h, pixels)?)
        }
        // CMYK32 needs an ICC-aware conversion and L16 is not worth a second
        // code path; both are rare and handled correctly by the full decode.
        _ => return None,
    };
    Some((img, (ow, oh)))
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

    /// A `w`x`h` scene written in the format `name`'s extension implies. Only
    /// low-frequency structure, because that is all pHash keys on.
    fn save_scene(dir: &Path, name: &str, (w, h): (u32, u32)) -> PathBuf {
        use image::Rgb;
        let mut img = RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let u = x as f32 / w as f32;
            let v = y as f32 / h as f32;
            let g = 128.0 + 90.0 * (u * 3.0 + v * 1.4).sin() * (v * 2.2 - u * 0.8).cos();
            let g = g.clamp(0.0, 255.0) as u8;
            *p = Rgb([g, (g / 2).saturating_add(40), 200u8.saturating_sub(g / 2)]);
        }
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    /// The scaled-JPEG decode is a memory/speed optimization, not a change of
    /// meaning: it must report the ORIGINAL dimensions and land within a few
    /// bits of the full decode. Measured drift on synthetic scenes is ~0-12
    /// bits of 256; 32 leaves headroom without letting a real regression
    /// (a wrong scale, a mangled buffer) slip through.
    #[test]
    fn scaled_jpeg_decode_matches_the_full_decode() {
        let dir = tempfile::tempdir().unwrap();
        // `choose_idct_size` only drops to 1/4 scale once the long edge reaches
        // ~4x WORK_SIZE, so 4x/3x is the cheapest source that still proves the
        // reduced IDCT ran.
        let source = (WORK_SIZE * 4, WORK_SIZE * 3);
        let path = save_scene(dir.path(), "big.jpg", source);

        // Guard against a vacuous test: prove the reduced IDCT actually ran
        // (a smaller buffer than the source) rather than silently decoding
        // full-size and passing for the wrong reason.
        let (decoded, source_dims) = decode_jpeg_scaled(&path).expect("jpeg fast path");
        assert_eq!(source_dims, source);
        assert!(
            decoded.dimensions().0 < source.0,
            "expected a scaled decode, got {:?}",
            decoded.dimensions()
        );

        // Reference: the original full-decode path. Both sides are hashed
        // here rather than through `hash_photo`, so the test proves the two
        // reconstructions agree no matter which one the default selects --
        // and never reads `SEQUIN_FULL_DECODE`, a process-global flag that
        // would race every other test in the binary.
        let reference = image::open(&path).unwrap();
        assert_eq!(
            source_dims,
            reference.dimensions(),
            "scaled decode must still report the source file's dimensions"
        );
        let (scaled_full, scaled_crop, _) = hashes_of(&to_work_image(decoded));
        let (ref_full, ref_crop, _) = hashes_of(&to_work_image(reference));

        let d_full = hamming(&scaled_full, &ref_full);
        let d_crop = hamming(&scaled_crop, &ref_crop);
        assert!(
            d_full <= 32 && d_crop <= 32,
            "scaled decode drifted too far from the full decode: \
             full {d_full}, cropped {d_crop} (of 256)"
        );
    }

    /// The fast path hands bytes straight to `jpeg_decoder`, so it owes the
    /// allocation guard `image::open` used to provide. The budget is checked
    /// against the frame header, before `decode` commits anything: a real
    /// 24MP photo passes with room to spare, while a header claiming the
    /// maximum 65535x65535 would demand ~25 GB and must be refused outright.
    #[test]
    fn oversized_frames_are_refused_before_decoding() {
        assert!(
            decode_budget_bytes(6000, 4000, 3) < MAX_DECODE_BYTES,
            "a 24MP RGB photo must stay inside the budget"
        );
        assert!(
            decode_budget_bytes(u16::MAX, u16::MAX, 3) > MAX_DECODE_BYTES,
            "a maximal frame must be refused"
        );
        // Rounding up to whole blocks must not overflow at the u16 ceiling.
        // 65535 rounds up to 8192 blocks, 1 to one block: 8192 * 64 * 2.
        assert_eq!(decode_budget_bytes(u16::MAX, 1, 1), 1_048_576);
    }

    /// Non-JPEG input has no DCT to scale, so it must fall through to the full
    /// decode rather than silently returning nothing — and that path must also
    /// report the source's dimensions, not the downscaled working copy's.
    /// Only needs to exceed WORK_SIZE, so it stays small enough to be cheap.
    #[test]
    fn png_falls_back_to_the_full_decode() {
        let dir = tempfile::tempdir().unwrap();
        let source = (WORK_SIZE + 200, WORK_SIZE);
        let path = save_scene(dir.path(), "big.png", source);
        assert!(decode_jpeg_scaled(&path).is_none(), "png has no fast path");
        let photo = hash_photo(&path).unwrap();
        assert_eq!((photo.width, photo.height), source);
    }

    /// The cap is the whole point of the bounded pool: it must hold on a
    /// many-core machine, never exceed the cores on a small one, and yield
    /// only to an explicit, usable override.
    #[test]
    fn scan_thread_count_is_capped_unless_overridden() {
        assert_eq!(resolve_scan_threads(None, 10), DEFAULT_MAX_DECODES);
        assert_eq!(resolve_scan_threads(None, 2), 2, "never exceeds the cores");
        assert_eq!(resolve_scan_threads(Some("12"), 10), 12, "override wins");
        for bad in ["0", "-1", "lots", "", "  "] {
            assert_eq!(
                resolve_scan_threads(Some(bad), 10),
                DEFAULT_MAX_DECODES,
                "{bad:?} is not a usable override"
            );
        }
    }

    /// `with_scan_pool` exists so a nested `par_iter` inherits the bounded
    /// pool instead of rayon's global one-task-per-core width. The assertion
    /// is one-sided (a scheduler that happens to run fewer workers still
    /// passes), so it cannot flake, but it does fail if the bound is lost.
    #[test]
    fn scan_pool_bounds_nested_parallel_work() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

        let live = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        with_scan_pool(|| {
            (0..512).into_par_iter().for_each(|_| {
                peak.fetch_max(live.fetch_add(1, SeqCst) + 1, SeqCst);
                std::thread::sleep(std::time::Duration::from_micros(100));
                live.fetch_sub(1, SeqCst);
            });
        })
        .unwrap();

        let peak = peak.load(SeqCst);
        assert!(peak > 0, "the pool ran nothing");
        assert!(
            peak <= scan_threads(),
            "pool ran {peak} concurrent tasks, cap is {}",
            scan_threads()
        );
    }

    /// Extension matching is case-insensitive and shared by the scan filter
    /// and the JPEG fast path, so an `.JPG` delivery can never be listed by
    /// one and skipped by the other.
    #[test]
    fn extension_matching_is_case_insensitive_and_shared() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbImage::from_fn(64, 48, |x, y| image::Rgb([x as u8, y as u8, 128]));
        for name in ["a.JPG", "b.jpeg", "c.PNG"] {
            img.save(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), "not a photo").unwrap();

        let listed: Vec<String> = list_photo_paths(dir.path())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(listed, ["a.JPG", "b.jpeg", "c.PNG"]);

        assert!(
            decode_jpeg_scaled(&dir.path().join("a.JPG")).is_some(),
            "uppercase .JPG must take the same fast path as .jpg"
        );
    }
}
