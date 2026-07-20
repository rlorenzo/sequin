//! Apply an arrangement's timeline to disk: copy-to-output or in-place EXIF
//! writing with per-file progress, failure tolerance, and read-back
//! verification. The GUI's write flow and the CLI's `apply` both run
//! through here.

use crate::exif;
use crate::timeline::TimedPhoto;
use anyhow::{ensure, Context, Result};
use chrono::NaiveDateTime;
use std::path::{Path, PathBuf};

/// Name of the copies folder created inside the delivery folder.
pub const OUTPUT_DIR_NAME: &str = "sequin-output";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// Copy each photo into `<folder>/sequin-output/` and stamp the copy;
    /// originals are never touched. The default.
    CopyToOutput,
    /// Stamp the originals (each write is still atomic temp+rename).
    InPlace,
}

#[derive(Debug, Clone)]
pub struct ApplyReport {
    /// Photos whose final file carries the new timestamp.
    pub written: usize,
    /// Per-file write failures (source path, error).
    pub failures: Vec<(PathBuf, String)>,
    /// Sampled read-back checks that matched.
    pub verified: usize,
    /// Sampled read-back checks that did NOT match (path, detail).
    pub verify_failures: Vec<(PathBuf, String)>,
    /// The output folder, when `CopyToOutput` was used.
    pub output_dir: Option<PathBuf>,
}

/// Default shoot start: the first photo's modification *date* at 10:00, per
/// PLAN.md M4 — deliveries carry a bogus capture time but a truthful mtime.
/// Falls back to today at 10:00 when the mtime is unreadable.
pub fn default_start(first_photo: &Path) -> NaiveDateTime {
    let date = std::fs::metadata(first_photo)
        .and_then(|m| m.modified())
        .ok()
        .map(chrono::DateTime::<chrono::Local>::from)
        .map(|dt| dt.date_naive())
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    date.and_hms_opt(10, 0, 0)
        .expect("10:00:00 is always a valid time")
}

/// Write every timestamp, reporting progress after each file. Failures are
/// collected, never fatal. After writing, a first/middle/last sample is read
/// back and compared.
pub fn apply(
    timed: &[TimedPhoto],
    folder: &Path,
    dest: Destination,
    progress: impl Fn(usize, usize),
) -> Result<ApplyReport> {
    let total = timed.len();
    let output_dir = match dest {
        Destination::CopyToOutput => {
            let dir = folder.join(OUTPUT_DIR_NAME);
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            Some(dir)
        }
        Destination::InPlace => None,
    };

    // Successfully stamped photos, at their final on-disk paths.
    let mut stamped: Vec<TimedPhoto> = Vec::with_capacity(total);
    let mut failures = Vec::new();
    // The output folder is flat, so two sources sharing a file name would
    // silently overwrite each other without this.
    let mut used_names = std::collections::HashSet::new();

    for (i, t) in timed.iter().enumerate() {
        let result = (|| -> Result<TimedPhoto> {
            let target = match &output_dir {
                Some(dir) => {
                    let name = t
                        .path
                        .file_name()
                        .with_context(|| format!("no file name in {}", t.path.display()))?;
                    let dest_path = dir.join(name);
                    // fs::copy onto the source truncates it to zero bytes.
                    // Compare canonical paths so a symlinked prefix (macOS
                    // /tmp → /private/tmp, or the folder reached two ways)
                    // can't smuggle a source past a lexical `!=`.
                    let canon =
                        |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                    ensure!(
                        canon(&dest_path) != canon(&t.path),
                        "{} is already inside {}",
                        t.path.display(),
                        OUTPUT_DIR_NAME
                    );
                    ensure!(
                        used_names.insert(name.to_os_string()),
                        "duplicate output name {}",
                        name.to_string_lossy()
                    );
                    std::fs::copy(&t.path, &dest_path)
                        .with_context(|| format!("copying {}", t.path.display()))?;
                    dest_path
                }
                None => t.path.clone(),
            };
            let photo = TimedPhoto {
                path: target,
                exif_datetime: t.exif_datetime.clone(),
            };
            exif::write_timestamp(&photo).inspect_err(|_| {
                // A half-made copy must not linger in the output folder — it
                // still carries the delivery's bogus timestamp.
                if photo.path != t.path {
                    let _ = std::fs::remove_file(&photo.path);
                }
            })?;
            Ok(photo)
        })();
        match result {
            Ok(photo) => stamped.push(photo),
            Err(e) => failures.push((t.path.clone(), format!("{e:#}"))),
        }
        progress(i + 1, total);
    }

    // Verify a first/middle/last sample of the successful writes. The
    // indices are ascending, so `dedup` collapses them for short runs.
    let mut verified = 0;
    let mut verify_failures = Vec::new();
    if !stamped.is_empty() {
        let mut sample: Vec<usize> = vec![0, stamped.len() / 2, stamped.len() - 1];
        sample.dedup();
        for idx in sample {
            let t = &stamped[idx];
            match exif::read_datetime_original(&t.path) {
                Ok(Some(read)) if read == t.exif_datetime => verified += 1,
                Ok(read) => verify_failures.push((
                    t.path.clone(),
                    format!(
                        "read back {:?}, expected {:?}",
                        read.unwrap_or_default(),
                        t.exif_datetime
                    ),
                )),
                Err(e) => verify_failures.push((t.path.clone(), format!("{e:#}"))),
            }
        }
    }

    Ok(ApplyReport {
        written: stamped.len(),
        failures,
        verified,
        verify_failures,
        output_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn save_photo(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        RgbImage::from_pixel(64, 64, Rgb([100, 110, 120]))
            .save(&p)
            .unwrap();
        p
    }

    fn timed(path: &Path, ts: &str) -> TimedPhoto {
        TimedPhoto {
            path: path.to_path_buf(),
            exif_datetime: ts.to_string(),
        }
    }

    #[test]
    fn copy_mode_stamps_copies_and_leaves_originals_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let a = save_photo(dir.path(), "a.jpg");
        let b = save_photo(dir.path(), "b.jpg");
        let timed = [
            timed(&a, "2026:07:18 10:00:00"),
            timed(&b, "2026:07:18 10:00:10"),
        ];

        let report = apply(&timed, dir.path(), Destination::CopyToOutput, |_, _| {}).unwrap();

        assert_eq!(report.written, 2);
        assert!(report.failures.is_empty());
        assert_eq!(report.verified, 2); // middle == last for n=2, deduped
        assert!(report.verify_failures.is_empty());

        let out = report.output_dir.expect("output dir");
        assert_eq!(out, dir.path().join(OUTPUT_DIR_NAME));
        let copy_a = exif::read_datetime_original(&out.join("a.jpg")).unwrap();
        assert_eq!(copy_a.as_deref(), Some("2026:07:18 10:00:00"));
        // Originals: still no EXIF timestamp.
        assert_eq!(exif::read_datetime_original(&a).unwrap(), None);
        assert_eq!(exif::read_datetime_original(&b).unwrap(), None);
    }

    #[test]
    fn in_place_mode_stamps_originals() {
        let dir = tempfile::tempdir().unwrap();
        let a = save_photo(dir.path(), "a.jpg");
        let report = apply(
            &[timed(&a, "2026:07:18 11:00:00")],
            dir.path(),
            Destination::InPlace,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(report.written, 1);
        assert!(report.output_dir.is_none());
        let read = exif::read_datetime_original(&a).unwrap();
        assert_eq!(read.as_deref(), Some("2026:07:18 11:00:00"));
    }

    #[test]
    fn failures_are_tolerated_and_progress_reaches_total() {
        let dir = tempfile::tempdir().unwrap();
        let good = save_photo(dir.path(), "good.jpg");
        let missing = dir.path().join("missing.jpg");
        let calls = std::cell::RefCell::new(Vec::new());

        let report = apply(
            &[
                timed(&missing, "2026:07:18 10:00:00"),
                timed(&good, "2026:07:18 10:00:10"),
            ],
            dir.path(),
            Destination::CopyToOutput,
            |done, total| calls.borrow_mut().push((done, total)),
        )
        .unwrap();

        assert_eq!(report.written, 1);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].0.ends_with("missing.jpg"));
        assert_eq!(*calls.borrow(), vec![(1, 2), (2, 2)]);
    }

    #[test]
    fn copy_mode_rejects_self_copy_and_duplicate_names() {
        let dir = tempfile::tempdir().unwrap();
        // A photo already inside the output folder must not be copied onto
        // itself (fs::copy would truncate it to zero bytes).
        let out = dir.path().join(OUTPUT_DIR_NAME);
        std::fs::create_dir_all(&out).unwrap();
        let inside = save_photo(&out, "a.jpg");
        let inside_bytes = std::fs::read(&inside).unwrap();
        // Two sources sharing a name collide in the flat output folder.
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let first = save_photo(dir.path(), "b.jpg");
        let dup = save_photo(&sub, "b.jpg");

        let report = apply(
            &[
                timed(&inside, "2026:07:18 10:00:00"),
                timed(&first, "2026:07:18 10:00:10"),
                timed(&dup, "2026:07:18 10:00:20"),
            ],
            dir.path(),
            Destination::CopyToOutput,
            |_, _| {},
        )
        .unwrap();

        assert_eq!(report.written, 1);
        assert_eq!(report.failures.len(), 2);
        assert!(report.failures[0].0.ends_with("a.jpg"));
        assert!(report.failures[1].0.ends_with(Path::new("sub/b.jpg")));
        // The self-copy source survives intact.
        assert_eq!(std::fs::read(&inside).unwrap(), inside_bytes);
    }

    #[test]
    fn failed_stamp_removes_the_copy_from_output() {
        let dir = tempfile::tempdir().unwrap();
        // Copies fine, but EXIF writing fails: not a real JPEG.
        let bad = dir.path().join("bad.jpg");
        std::fs::write(&bad, b"not a jpeg").unwrap();

        let report = apply(
            &[timed(&bad, "2026:07:18 10:00:00")],
            dir.path(),
            Destination::CopyToOutput,
            |_, _| {},
        )
        .unwrap();

        assert_eq!(report.written, 0);
        assert_eq!(report.failures.len(), 1);
        // The unstamped copy must not linger where it would get imported.
        assert!(!dir.path().join(OUTPUT_DIR_NAME).join("bad.jpg").exists());
        // The original is untouched.
        assert_eq!(std::fs::read(&bad).unwrap(), b"not a jpeg");
    }

    #[test]
    fn default_start_is_mtime_date_at_ten() {
        let dir = tempfile::tempdir().unwrap();
        let p = save_photo(dir.path(), "a.jpg");
        let start = default_start(&p);
        // The file was just created: its mtime date is today.
        assert_eq!(start.date(), chrono::Local::now().date_naive());
        assert_eq!(
            start.time(),
            chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap()
        );
    }
}
