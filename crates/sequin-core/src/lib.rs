//! Sequin core: group styled variants of the same shot, order them, and write
//! sequential EXIF capture times so Apple Photos displays them in order.
//!
//! Pipeline: scan → hash ([`hashing`]) → cluster ([`grouping`]) → user ordering
//! → timestamp assignment ([`timeline`]) → EXIF write ([`exif`]).
//!
//! Validated against a real 62-photo studio delivery: pHash 16×16 on the full
//! image plus a border-cropped variant, clustered at Hamming distance ≤ 60/256,
//! grouped every styled variant (B&W conversions, text overlays, background
//! swaps) with zero false merges. Alternate crops are NOT auto-matched (pHash
//! is not crop-tolerant) — the GUI must allow dragging strays into groups.

pub mod exif;
pub mod grouping;
pub mod hashing;
pub mod thumbs;
pub mod timeline;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One photo on disk plus its computed hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Photo {
    pub path: PathBuf,
    /// pHash (DCT) of the full image, 16x16 = 256 bits, hex-encoded.
    pub hash_full: String,
    /// pHash of the image after trimming uniform-color borders.
    pub hash_cropped: String,
    /// Fraction of area removed by border trimming (0.0 = no border found).
    pub border_fraction: f32,
    pub width: u32,
    pub height: u32,
}

/// A cluster of visually-matching variants of the same shot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub photos: Vec<Photo>,
}

/// The user's final arrangement: groups in timeline order, photos ordered
/// within each group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arrangement {
    pub groups: Vec<Group>,
}

impl Arrangement {
    pub fn photo_count(&self) -> usize {
        self.groups.iter().map(|g| g.photos.len()).sum()
    }
}
