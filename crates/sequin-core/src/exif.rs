//! EXIF timestamp writing via `little_exif`.
//!
//! Writes `DateTimeOriginal` (what Apple Photos sorts by), `CreateDate`
//! (DateTimeDigitized) and `ModifyDate` together so every consumer agrees.

use crate::timeline::TimedPhoto;
use anyhow::{Context, Result};
use little_exif::exif_tag::ExifTag;
use little_exif::metadata::Metadata;
use std::path::Path;

/// Does this `new_from_path` error mean "the file simply has no EXIF"?
/// `little_exif` 0.6.23 signals that case only via these message strings
/// (jpg.rs: "No EXIF data found!", png/mod.rs: "No metadata found!") — there
/// is no error enum to match on. Brittle, but failing closed on an
/// unrecognized error is strictly safer than the alternative below.
fn is_no_exif_error(e: &std::io::Error) -> bool {
    let msg = e.to_string();
    msg.contains("No EXIF data found") || msg.contains("No metadata found")
}

/// Write the assigned timestamp into one photo's EXIF, in place.
pub fn write_timestamp(photo: &TimedPhoto) -> Result<()> {
    let path: &Path = &photo.path;
    // Files with no EXIF segment at all (rare, but possible) start fresh.
    // Any OTHER read failure must propagate: writing a fresh Metadata
    // replaces the whole EXIF block, wiping tags we merely failed to read.
    let mut metadata = match Metadata::new_from_path(path) {
        Ok(m) => m,
        Err(e) if is_no_exif_error(&e) => Metadata::new(),
        Err(e) => return Err(e).with_context(|| format!("reading EXIF from {}", path.display())),
    };

    let ts = photo.exif_datetime.clone();
    metadata.set_tag(ExifTag::DateTimeOriginal(ts.clone()));
    metadata.set_tag(ExifTag::CreateDate(ts.clone()));
    metadata.set_tag(ExifTag::ModifyDate(ts));

    // Write to a sibling temp copy and rename over the original so an
    // interrupted write can never corrupt the only copy of a photo. The
    // temp name keeps the real extension (little_exif dispatches on it).
    let file_name = path
        .file_name()
        .with_context(|| format!("no file name in {}", path.display()))?;
    let tmp = path.with_file_name(format!("~sequin.{}", file_name.to_string_lossy()));
    std::fs::copy(path, &tmp).with_context(|| format!("copying {}", path.display()))?;
    let written = metadata
        .write_to_file(&tmp)
        .with_context(|| format!("writing EXIF to {}", path.display()))
        .and_then(|_| {
            std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Write timestamps for a whole arrangement. Returns paths that failed.
pub fn write_all(timed: &[TimedPhoto]) -> Vec<(std::path::PathBuf, String)> {
    let mut failures = Vec::new();
    for t in timed {
        if let Err(e) = write_timestamp(t) {
            failures.push((t.path.clone(), format!("{e:#}")));
        }
    }
    failures
}

/// Read back DateTimeOriginal (for dry-run verification and tests).
pub fn read_datetime_original(path: &Path) -> Result<Option<String>> {
    let Ok(metadata) = Metadata::new_from_path(path) else {
        return Ok(None); // no EXIF segment at all
    };
    for tag in &metadata {
        if let ExifTag::DateTimeOriginal(v) = tag {
            return Ok(Some(v.trim_end_matches('\0').to_string()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TimedPhoto;
    use image::{Rgb, RgbImage};

    #[test]
    fn roundtrip_datetime_original() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jpg");
        RgbImage::from_pixel(64, 64, Rgb([120, 130, 140]))
            .save(&p)
            .unwrap();

        let timed = TimedPhoto {
            path: p.clone(),
            exif_datetime: "2026:07:18 10:00:00".to_string(),
        };
        write_timestamp(&timed).unwrap();

        let read = read_datetime_original(&p).unwrap();
        assert_eq!(read.as_deref(), Some("2026:07:18 10:00:00"));
    }

    /// PNGs report "no EXIF yet" with a different error string than JPEGs;
    /// this pins the `is_no_exif_error` match for both supported formats.
    #[test]
    fn roundtrip_png_without_exif() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.png");
        RgbImage::from_pixel(64, 64, Rgb([120, 130, 140]))
            .save(&p)
            .unwrap();

        let timed = TimedPhoto {
            path: p.clone(),
            exif_datetime: "2026:07:18 10:00:00".to_string(),
        };
        write_timestamp(&timed).unwrap();

        let read = read_datetime_original(&p).unwrap();
        assert_eq!(read.as_deref(), Some("2026:07:18 10:00:00"));
    }

    #[test]
    fn write_all_reports_failures_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.jpg");
        RgbImage::from_pixel(64, 64, Rgb([90, 90, 90]))
            .save(&good)
            .unwrap();
        let missing = dir.path().join("no-such-dir").join("missing.jpg");

        let ts = |path: &std::path::Path| TimedPhoto {
            path: path.to_path_buf(),
            exif_datetime: "2026:07:18 10:00:00".to_string(),
        };
        // Failing photo first: the good one after it must still be written.
        let failures = write_all(&[ts(&missing), ts(&good)]);

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, missing);
        let read = read_datetime_original(&good).unwrap();
        assert_eq!(read.as_deref(), Some("2026:07:18 10:00:00"));
    }
}
