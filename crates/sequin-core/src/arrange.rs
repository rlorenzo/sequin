//! Arrangement editing operations and sidecar persistence.
//!
//! Pure functions over [`Arrangement`] so the GUI's drag/keyboard layer stays
//! thin and every edge case is testable headlessly. The sidecar file
//! (`arrangement.json`, written into the source folder) uses the exact same
//! schema as `sequin group` output, so it doubles as the CLI interchange and
//! as session resume.

use crate::{Arrangement, Group, Photo};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// File name of the sidecar arrangement in a delivery folder.
pub const SIDECAR_NAME: &str = "arrangement.json";

pub fn sidecar_path(folder: &Path) -> PathBuf {
    folder.join(SIDECAR_NAME)
}

/// Move the group at `from` so it ends up at index `to` (indices in the
/// current group list). Out-of-range indices are clamped; a no-op move is
/// fine.
pub fn move_group(arr: &mut Arrangement, from: usize, to: usize) {
    if arr.groups.is_empty() || from >= arr.groups.len() {
        return;
    }
    let group = arr.groups.remove(from);
    let to = to.min(arr.groups.len());
    arr.groups.insert(to, group);
}

/// Move one photo from `(from_group, from_photo)` to group `to_group`,
/// inserted at photo index `to_photo` (clamped). Works within one group or
/// across groups; indices refer to the state BEFORE removal. A group left
/// empty by the move is removed.
pub fn move_photo(arr: &mut Arrangement, from: (usize, usize), to_group: usize, to_photo: usize) {
    let (fg, fp) = from;
    if fg >= arr.groups.len() || fp >= arr.groups[fg].photos.len() || to_group >= arr.groups.len() {
        return;
    }
    let photo = arr.groups[fg].photos.remove(fp);

    // Same-group move: removal shifts later indices left by one.
    let mut tp = to_photo;
    if to_group == fg && to_photo > fp {
        tp -= 1;
    }
    let tp = tp.min(arr.groups[to_group].photos.len());
    arr.groups[to_group].photos.insert(tp, photo);

    if arr.groups[fg].photos.is_empty() {
        arr.groups.remove(fg);
    }
}

/// Take one photo out of its group and make it a new singleton group placed
/// at group index `dest_group` (clamped; indices refer to the state BEFORE
/// removal). Used both by explicit "split photo out" and by dropping a photo
/// into the gap between groups. Splitting a photo that is already alone in
/// its group just relocates that group.
pub fn split_photo(arr: &mut Arrangement, from: (usize, usize), dest_group: usize) {
    let (fg, fp) = from;
    if fg >= arr.groups.len() || fp >= arr.groups[fg].photos.len() {
        return;
    }
    if arr.groups[fg].photos.len() == 1 {
        move_group(arr, fg, dest_group.min(arr.groups.len().saturating_sub(1)));
        return;
    }
    let photo = arr.groups[fg].photos.remove(fp);
    let dest = dest_group.min(arr.groups.len());
    arr.groups.insert(
        dest,
        Group {
            photos: vec![photo],
        },
    );
}

/// Merge the photos of `source` onto the end of `target`; `source` is
/// removed. Indices refer to the state before the merge.
pub fn merge_groups(arr: &mut Arrangement, target: usize, source: usize) {
    if target == source || target >= arr.groups.len() || source >= arr.groups.len() {
        return;
    }
    let photos = std::mem::take(&mut arr.groups[source].photos);
    arr.groups[target].photos.extend(photos);
    arr.groups.remove(source);
}

/// Locate a photo by path, returning `(group index, photo index)`. Edits
/// shuffle indices, so the GUI re-finds things by path after every mutation.
pub fn find_photo(arr: &Arrangement, path: &Path) -> Option<(usize, usize)> {
    arr.groups.iter().enumerate().find_map(|(gi, g)| {
        g.photos
            .iter()
            .position(|p| p.path == path)
            .map(|pi| (gi, pi))
    })
}

/// True when the arrangement covers exactly the given photo paths (same set,
/// any order) — the gate for resuming a saved sidecar after a rescan.
pub fn covers_same_photos(arr: &Arrangement, photos: &[Photo]) -> bool {
    // Set equality alone would let a hand-edited duplicate entry through.
    if arr.photo_count() != photos.len() {
        return false;
    }
    let saved: BTreeSet<&Path> = arr
        .groups
        .iter()
        .flat_map(|g| g.photos.iter())
        .map(|p| p.path.as_path())
        .collect();
    let scanned: BTreeSet<&Path> = photos.iter().map(|p| p.path.as_path()).collect();
    saved == scanned
}

/// Atomically write the arrangement sidecar (temp + rename, like all other
/// writes that must never leave a torn file). The temp name is unique per
/// call so overlapping saves can never truncate or unlink each other's file;
/// callers that need write ORDER (last edit wins) must still serialize.
pub fn save(arr: &Arrangement, folder: &Path) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dest = sidecar_path(folder);
    let tmp = dest.with_extension(format!(
        "json.tmp{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let json = serde_json::to_vec_pretty(arr)?;
    let write = std::fs::write(&tmp, &json)
        .and_then(|_| std::fs::rename(&tmp, &dest))
        .with_context(|| format!("saving {}", dest.display()));
    if write.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write
}

/// Load the sidecar if present. `Ok(None)` when the file doesn't exist;
/// `Err` for a present-but-unreadable file (surfaced, never silently
/// ignored).
pub fn load(folder: &Path) -> Result<Option<Arrangement>> {
    let path = sidecar_path(folder);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut arr: Arrangement =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    // A hand-edited sidecar may contain empty groups; never let one reach
    // consumers that assume every group has a first photo.
    arr.groups.retain(|g| !g.photos.is_empty());
    Ok(Some(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(name: &str) -> Photo {
        Photo {
            path: PathBuf::from(format!("/d/{name}.jpg")),
            hash_full: "0".repeat(64),
            hash_cropped: "0".repeat(64),
            border_fraction: 0.0,
            width: 100,
            height: 100,
        }
    }

    /// Groups from a spec like `[["a","b"],["c"]]`.
    fn arr(spec: &[&[&str]]) -> Arrangement {
        Arrangement {
            groups: spec
                .iter()
                .map(|names| Group {
                    photos: names.iter().map(|n| photo(n)).collect(),
                })
                .collect(),
        }
    }

    /// Flatten back to a spec for easy assertions.
    fn spec(arr: &Arrangement) -> Vec<Vec<String>> {
        arr.groups
            .iter()
            .map(|g| {
                g.photos
                    .iter()
                    .map(|p| p.path.file_stem().unwrap().to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn move_group_reorders_and_clamps() {
        let mut a = arr(&[&["a"], &["b"], &["c"]]);
        move_group(&mut a, 0, 2);
        assert_eq!(spec(&a), [vec!["b"], vec!["c"], vec!["a"]]);
        move_group(&mut a, 2, 0);
        assert_eq!(spec(&a), [vec!["a"], vec!["b"], vec!["c"]]);
        move_group(&mut a, 1, 99); // clamped to end
        assert_eq!(spec(&a), [vec!["a"], vec!["c"], vec!["b"]]);
        move_group(&mut a, 99, 0); // out-of-range source: no-op
        assert_eq!(spec(&a), [vec!["a"], vec!["c"], vec!["b"]]);
    }

    #[test]
    fn move_photo_within_group_accounts_for_removal_shift() {
        let mut a = arr(&[&["a", "b", "c"]]);
        move_photo(&mut a, (0, 0), 0, 3); // a to the end
        assert_eq!(spec(&a), [vec!["b", "c", "a"]]);
        move_photo(&mut a, (0, 2), 0, 0); // a back to the front
        assert_eq!(spec(&a), [vec!["a", "b", "c"]]);
        move_photo(&mut a, (0, 0), 0, 1); // insert-before-b after removing a = no-op position
        assert_eq!(spec(&a), [vec!["a", "b", "c"]]);
    }

    #[test]
    fn move_photo_between_groups_removes_emptied_group() {
        let mut a = arr(&[&["a"], &["b", "c"]]);
        move_photo(&mut a, (0, 0), 1, 1);
        assert_eq!(spec(&a), [vec!["b", "a", "c"]]);
    }

    #[test]
    fn split_photo_creates_singleton_and_relocates_lone_photos() {
        let mut a = arr(&[&["a", "b"], &["c"]]);
        split_photo(&mut a, (0, 1), 1); // b out, placed between the groups
        assert_eq!(spec(&a), [vec!["a"], vec!["b"], vec!["c"]]);
        split_photo(&mut a, (1, 0), 0); // b is already a singleton: just moves
        assert_eq!(spec(&a), [vec!["b"], vec!["a"], vec!["c"]]);
    }

    #[test]
    fn merge_groups_appends_and_ignores_bad_input() {
        let mut a = arr(&[&["a"], &["b"], &["c"]]);
        merge_groups(&mut a, 0, 2);
        assert_eq!(spec(&a), [vec!["a", "c"], vec!["b"]]);
        merge_groups(&mut a, 1, 1); // self-merge: no-op
        assert_eq!(spec(&a), [vec!["a", "c"], vec!["b"]]);
        merge_groups(&mut a, 0, 99); // out of range: no-op
        assert_eq!(spec(&a), [vec!["a", "c"], vec!["b"]]);
    }

    #[test]
    fn find_photo_locates_by_path() {
        let a = arr(&[&["a", "b"], &["c"]]);
        assert_eq!(find_photo(&a, Path::new("/d/b.jpg")), Some((0, 1)));
        assert_eq!(find_photo(&a, Path::new("/d/c.jpg")), Some((1, 0)));
        assert_eq!(find_photo(&a, Path::new("/d/missing.jpg")), None);
    }

    #[test]
    fn sidecar_roundtrip_and_photo_set_gate() {
        let dir = tempfile::tempdir().unwrap();
        let a = arr(&[&["a", "b"], &["c"]]);
        save(&a, dir.path()).unwrap();
        let loaded = load(dir.path()).unwrap().expect("sidecar present");
        assert_eq!(spec(&loaded), spec(&a));

        let same_photos: Vec<Photo> = ["c", "a", "b"].iter().map(|n| photo(n)).collect();
        assert!(covers_same_photos(&loaded, &same_photos));
        let different: Vec<Photo> = ["a", "b"].iter().map(|n| photo(n)).collect();
        assert!(!covers_same_photos(&loaded, &different));

        assert!(load(tempfile::tempdir().unwrap().path()).unwrap().is_none());
    }

    #[test]
    fn load_drops_empty_groups_from_hand_edited_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = arr(&[&["a"], &["b"]]);
        a.groups.insert(1, Group { photos: vec![] });
        std::fs::write(sidecar_path(dir.path()), serde_json::to_vec(&a).unwrap()).unwrap();
        let loaded = load(dir.path()).unwrap().expect("sidecar present");
        assert_eq!(spec(&loaded), [vec!["a"], vec!["b"]]);
    }

    #[test]
    fn covers_same_photos_rejects_duplicate_entries() {
        let duplicated = arr(&[&["a"], &["a"]]);
        let scanned = vec![photo("a")];
        assert!(!covers_same_photos(&duplicated, &scanned));
    }

    #[test]
    fn load_surfaces_unreadable_sidecar_as_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(sidecar_path(dir.path()), b"{not json").unwrap();
        assert!(load(dir.path()).is_err());
    }
}
