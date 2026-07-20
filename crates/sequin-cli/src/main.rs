//! Sequin CLI — exercises the core pipeline without the GUI.
//!
//! Usage:
//!   sequin group <dir>                          print detected groups as JSON
//!   sequin apply <arrangement.json> <start> [--dry-run] [--in-place]
//!                                               assign times & write EXIF
//!     <start> format: "2026-07-18 10:00"
//!     By default stamps copies in <folder>/sequin-output/ (folder = the
//!     first photo's parent directory); --in-place stamps the originals.
//!
//! `apply` expects the JSON produced by `group` (edit the order to taste —
//! this is exactly what the GUI will do interactively).

use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use sequin_core::{apply, grouping, hashing, timeline, Arrangement};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("group") => {
            let dir = args.get(1).context("usage: sequin group <dir>")?;
            cmd_group(Path::new(dir))
        }
        Some("apply") => {
            let file = args
                .get(1)
                .context("usage: sequin apply <arrangement.json> <start>")?;
            let start = args
                .get(2)
                .context("missing <start>, e.g. \"2026-07-18 10:00\"")?;
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let in_place = args.iter().any(|a| a == "--in-place");
            cmd_apply(Path::new(file), start, dry_run, in_place)
        }
        _ => {
            eprintln!(
                "usage: sequin group <dir> | sequin apply <arrangement.json> <start> [--dry-run] [--in-place]"
            );
            std::process::exit(2);
        }
    }
}

fn cmd_group(dir: &Path) -> Result<()> {
    let photos = hashing::scan_dir(dir)?;
    eprintln!("hashed {} photos", photos.len());
    let groups = grouping::cluster(&photos, grouping::DEFAULT_THRESHOLD)?;
    eprintln!(
        "{} groups ({} multi-photo, {} singletons)",
        groups.len(),
        groups.iter().filter(|g| g.photos.len() > 1).count(),
        groups.iter().filter(|g| g.photos.len() == 1).count()
    );
    let arrangement = Arrangement { groups };
    println!("{}", serde_json::to_string_pretty(&arrangement)?);
    Ok(())
}

fn cmd_apply(file: &Path, start: &str, dry_run: bool, in_place: bool) -> Result<()> {
    let arrangement: Arrangement = serde_json::from_reader(
        std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?,
    )?;
    let start = NaiveDateTime::parse_from_str(&format!("{start}:00"), "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M:%S"))
        .context("start time must look like \"2026-07-18 10:00\"")?;

    let timed = timeline::assign_times(&arrangement, start, timeline::Spacing::default());
    for t in &timed {
        println!("{}  {}", t.exif_datetime, t.path.display());
    }
    if dry_run {
        eprintln!("dry run: no files modified");
        return Ok(());
    }

    let folder = timed
        .first()
        .and_then(|t| t.path.parent())
        .context("arrangement contains no photos")?
        .to_path_buf();
    let dest = if in_place {
        apply::Destination::InPlace
    } else {
        apply::Destination::CopyToOutput
    };
    let report = apply::apply(&timed, &folder, dest, |done, total| {
        eprint!("\rwriting {done}/{total}");
    })?;
    eprintln!();

    for (p, e) in &report.failures {
        eprintln!("FAILED {}: {}", p.display(), e);
    }
    for (p, e) in &report.verify_failures {
        eprintln!("VERIFY FAILED {}: {}", p.display(), e);
    }
    if let Some(dir) = &report.output_dir {
        eprintln!("stamped copies in {}", dir.display());
    }
    eprintln!(
        "wrote {} of {} files, verified {} read-back sample(s)",
        report.written,
        timed.len(),
        report.verified
    );
    if !report.failures.is_empty() || !report.verify_failures.is_empty() {
        bail!(
            "{} write failure(s), {} verification failure(s)",
            report.failures.len(),
            report.verify_failures.len()
        );
    }
    Ok(())
}
