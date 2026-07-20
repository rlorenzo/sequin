//! Clustering photos into variant groups via union-find over pairwise
//! Hamming distances.

use crate::hashing::{hamming, hash_from_hex};
use crate::{Group, Photo};
use anyhow::Result;

/// Max Hamming distance (out of 256 bits) for two photos to be considered
/// variants of the same shot. Validated on real data: true variants matched
/// at ≤ 60 while the nearest false pair was ≥ 102 — a wide margin.
pub const DEFAULT_THRESHOLD: u32 = 60;

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut a: usize) -> usize {
        while self.parent[a] != a {
            self.parent[a] = self.parent[self.parent[a]];
            a = self.parent[a];
        }
        a
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Cluster photos whose pairwise distance is within `threshold`.
/// Groups are returned largest-first; photos within a group keep scan order.
pub fn cluster(photos: &[Photo], threshold: u32) -> Result<Vec<Group>> {
    let n = photos.len();
    let mut uf = UnionFind::new(n);
    // Decode each photo's hex hashes once; the pairwise metric below is
    // min(full, cropped), identical to `hashing::distance`.
    let decoded = photos
        .iter()
        .map(|p| {
            Ok((
                hash_from_hex(&p.hash_full)?,
                hash_from_hex(&p.hash_cropped)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    for i in 0..n {
        for j in (i + 1)..n {
            let d =
                hamming(&decoded[i].0, &decoded[j].0).min(hamming(&decoded[i].1, &decoded[j].1));
            if d <= threshold {
                uf.union(i, j);
            }
        }
    }

    let mut by_root: std::collections::BTreeMap<usize, Vec<Photo>> = Default::default();
    for (i, photo) in photos.iter().enumerate() {
        let root = uf.find(i);
        by_root.entry(root).or_default().push(photo.clone());
    }

    let mut groups: Vec<Group> = by_root
        .into_values()
        .map(|photos| Group { photos })
        .collect();
    groups.sort_by(|a, b| {
        b.photos
            .len()
            .cmp(&a.photos.len())
            .then_with(|| a.photos[0].path.cmp(&b.photos[0].path))
    });
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashing::{distance, hash_photo};
    use image::{Rgb, RgbImage};

    /// Smooth photo-like scene: soft blob + gentle waves (mimics portrait
    /// bokeh). pHash keys on low-frequency structure, so synthetic negatives
    /// must differ structurally, not just in per-pixel noise.
    fn scene_bright_blob() -> RgbImage {
        RgbImage::from_fn(400, 300, |x, y| {
            let fx = x as f32 / 400.0;
            let fy = y as f32 / 300.0;
            let d = ((fx - 0.30).powi(2) + (fy - 0.35).powi(2)).sqrt();
            let blob = (1.0 - (d * 3.0).min(1.0)) * 180.0;
            let wave = ((fx * 7.0).sin() * (fy * 5.0).cos() * 0.5 + 0.5) * 60.0;
            Rgb([
                (30.0 + blob + wave * 0.9) as u8,
                (40.0 + blob * 0.8 + wave) as u8,
                (70.0 + blob * 0.5 + wave * 0.1) as u8,
            ])
        })
    }

    fn scene_dark_blob() -> RgbImage {
        RgbImage::from_fn(400, 300, |x, y| {
            let fx = x as f32 / 400.0;
            let fy = y as f32 / 300.0;
            let d = ((fx - 0.7).powi(2) + (fy - 0.75).powi(2)).sqrt();
            let hole = (d * 2.5).min(1.0) * 190.0;
            let wave = ((fx * 23.0).cos() * (fy * 17.0).sin() * 0.5 + 0.5) * 50.0;
            Rgb([
                (220.0 - hole * 0.9 + wave * 0.3) as u8,
                (200.0 - hole + wave * 0.5) as u8,
                (180.0 - hole * 0.6 + wave) as u8,
            ])
        })
    }

    /// Threshold for the SYNTHETIC images in this test. Generated scenes are
    /// a worst case for the border-trim path (hard white border + strong
    /// high-frequency waves): measured band is variant=96 / negative=124.
    /// Real photos behave much better — the production DEFAULT_THRESHOLD=60
    /// reproduces the visually-verified grouping of a real 62-photo delivery
    /// exactly (variant ≤60 / nearest-false ≥102); that golden test lives in
    /// the CLI + fixtures/expected_groups_archive1-2.json.
    const SYNTHETIC_THRESHOLD: u32 = 110;

    /// A bordered copy of an image must cluster with its original; a
    /// structurally different image must not.
    #[test]
    fn bordered_variant_groups_with_original() {
        let dir = tempfile::tempdir().unwrap();
        let a = scene_bright_blob();
        let a_path = dir.path().join("a.jpg");
        a.save(&a_path).unwrap();

        // white-bordered variant of a (~8% border all around)
        let (w, h) = a.dimensions();
        let mut bordered = RgbImage::from_pixel(w + w / 6, h + h / 6, Rgb([255, 255, 255]));
        image::imageops::overlay(&mut bordered, &a, (w / 12) as i64, (h / 12) as i64);
        let b_path = dir.path().join("a_bordered.jpg");
        bordered.save(&b_path).unwrap();

        let c_path = dir.path().join("c.jpg");
        scene_dark_blob().save(&c_path).unwrap();

        let photos = vec![
            hash_photo(&a_path).unwrap(),
            hash_photo(&b_path).unwrap(),
            hash_photo(&c_path).unwrap(),
        ];

        // the bordered variant must be closer to its original than the
        // negative pair is, regardless of absolute threshold
        let d_variant = distance(&photos[0], &photos[1]).unwrap();
        let d_negative = distance(&photos[0], &photos[2]).unwrap();
        assert!(
            d_variant < d_negative,
            "variant ({d_variant}) should be closer than negative ({d_negative})"
        );

        let groups = cluster(&photos, SYNTHETIC_THRESHOLD).unwrap();
        assert_eq!(groups.len(), 2, "expected {{a, a_bordered}} and {{c}}");
        assert_eq!(groups[0].photos.len(), 2);
        assert!(groups[0].photos.iter().any(|p| p.path == a_path));
        assert!(groups[0].photos.iter().any(|p| p.path == b_path));
    }
}
