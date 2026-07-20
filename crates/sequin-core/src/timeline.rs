//! Assign sequential capture times to an ordered arrangement.

use crate::Arrangement;
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Spacing defaults chosen so the Apple Photos timeline reads naturally:
/// groups a minute apart, variants ten seconds apart within a group.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Spacing {
    pub between_groups_secs: i64,
    pub within_group_secs: i64,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            between_groups_secs: 60,
            within_group_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedPhoto {
    pub path: PathBuf,
    /// EXIF-format timestamp, e.g. "2026:07:18 10:03:20".
    pub exif_datetime: String,
}

/// Walk the arrangement in order and assign a timestamp to every photo.
/// The first photo of the first group gets exactly `start`.
pub fn assign_times(
    arrangement: &Arrangement,
    start: NaiveDateTime,
    spacing: Spacing,
) -> Vec<TimedPhoto> {
    let mut out = Vec::with_capacity(arrangement.photo_count());
    let mut group_start = start;
    for group in &arrangement.groups {
        let mut t = group_start;
        for photo in &group.photos {
            out.push(TimedPhoto {
                path: photo.path.clone(),
                exif_datetime: t.format("%Y:%m:%d %H:%M:%S").to_string(),
            });
            t += Duration::seconds(spacing.within_group_secs);
        }
        // next group starts a fixed gap after this group's *last* photo
        group_start =
            t + Duration::seconds(spacing.between_groups_secs - spacing.within_group_secs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Group, Photo};

    fn photo(name: &str) -> Photo {
        Photo {
            path: PathBuf::from(name),
            hash_full: String::new(),
            hash_cropped: String::new(),
            border_fraction: 0.0,
            width: 0,
            height: 0,
        }
    }

    #[test]
    fn times_are_sequential_and_spaced() {
        let arr = Arrangement {
            groups: vec![
                Group {
                    photos: vec![photo("a"), photo("b")],
                },
                Group {
                    photos: vec![photo("c")],
                },
            ],
        };
        let start =
            NaiveDateTime::parse_from_str("2026-07-18 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let timed = assign_times(&arr, start, Spacing::default());
        let stamps: Vec<&str> = timed.iter().map(|t| t.exif_datetime.as_str()).collect();
        assert_eq!(
            stamps,
            vec![
                "2026:07:18 10:00:00", // group 1, photo 1
                "2026:07:18 10:00:10", // group 1, photo 2 (+10s)
                "2026:07:18 10:01:10", // group 2 starts +60s after last photo
            ]
        );
    }
}
