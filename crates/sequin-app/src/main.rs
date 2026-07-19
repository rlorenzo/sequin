//! Sequin desktop app (Dioxus).
//!
//! M2: grouped thumbnail grid. Pick a folder → hash + thumbnail every photo
//! (one decode each, rayon-parallel, progress streamed to the UI) → cluster →
//! render each group as a row of thumbnails on the light table.
//! See PLAN.md M3–M4 for drag-to-reorder and the EXIF-write flow.

use dioxus::desktop::wry::http::Response;
use dioxus::desktop::{use_asset_handler, Config, WindowBuilder};
use dioxus::prelude::*;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sequin_core::{grouping, thumbs, Arrangement};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

/// Escape everything but RFC 3986 unreserved characters in thumbnail URLs.
const URL_ESCAPED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn main() {
    let window = WindowBuilder::new()
        .with_title("Sequin")
        .with_inner_size(dioxus::desktop::LogicalSize::new(1060.0, 760.0))
        .with_min_inner_size(dioxus::desktop::LogicalSize::new(680.0, 480.0));
    dioxus::LaunchBuilder::new()
        .with_cfg(Config::new().with_window(window))
        .launch(app);
}

/// Everything the grid needs about a completed scan.
#[derive(Clone)]
struct Session {
    folder: PathBuf,
    arrangement: Arrangement,
    /// URL prefix thumbnails are served under: `/thumbs/<cache-key>`.
    thumb_base: String,
    is_bw: HashMap<PathBuf, bool>,
    failures: Vec<(PathBuf, String)>,
}

#[derive(Clone)]
enum Phase {
    Idle,
    Scanning { done: usize, total: usize },
    Grouping,
    Ready(Session),
    Error(String),
}

/// Progress messages sent from the blocking scan into the UI.
enum Prog {
    Scan(usize, usize),
    Grouping,
}

/// Per-group view model, precomputed so the RSX stays plain.
struct GroupView {
    key: String,
    index: String,
    stagger: String,
    meta: String,
    photos: Vec<PhotoView>,
}

struct PhotoView {
    key: String,
    src: String,
    name: String,
    aspect: String,
}

fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("sequin")
        .join("thumbs")
}

fn app() -> Element {
    let phase = use_signal(|| Phase::Idle);

    // Serve cached thumbnails to the webview: /thumbs/<cache-key>/<file>.jpg
    // maps to <cache_root>/<cache-key>/<file>.jpg. Stateless; traversal-safe.
    use_asset_handler("thumbs", move |request, responder| {
        let root = cache_root();
        let path = request.uri().path().to_string();
        let rel = percent_decode_str(path.strip_prefix("/thumbs/").unwrap_or(""))
            .decode_utf8_lossy()
            .into_owned();
        let safe = !rel.is_empty()
            && rel
                .split('/')
                .all(|c| !c.is_empty() && c != "." && c != "..");
        let reply = if safe {
            std::fs::read(root.join(&rel)).ok()
        } else {
            None
        };
        let response = match reply {
            Some(bytes) => Response::builder()
                .status(200)
                .header("Content-Type", "image/jpeg")
                .body(Cow::from(bytes))
                .unwrap(),
            None => Response::builder()
                .status(404)
                .body(Cow::from(Vec::new()))
                .unwrap(),
        };
        responder.respond(response);
    });

    // Dev hook: SEQUIN_OPEN=<folder> scans immediately on launch.
    use_hook(move || {
        if let Ok(dir) = std::env::var("SEQUIN_OPEN") {
            start_scan(phase, PathBuf::from(dir));
        }
    });

    let pick_folder = move |_| {
        spawn(async move {
            if let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await {
                start_scan(phase, dir.path().to_path_buf());
            }
        });
    };

    let current = phase.read().clone();
    let busy = matches!(current, Phase::Scanning { .. } | Phase::Grouping);
    let summary = match &current {
        Phase::Ready(s) if !s.arrangement.groups.is_empty() => Some(format!(
            "{} · {}",
            plural(s.arrangement.photo_count(), "photo", "photos"),
            plural(s.arrangement.groups.len(), "group", "groups")
        )),
        _ => None,
    };

    let content = match &current {
        Phase::Idle => rsx! {
            div { class: "stage",
                h2 { "Put your shoot back in order." }
                p { class: "lede",
                    "Open a studio delivery and Sequin groups the styled variants "
                    "of each shot, ready to arrange and timestamp for Apple Photos."
                }
                button { class: "btn primary", onclick: pick_folder, "Open photo folder…" }
            }
        },
        Phase::Scanning { done, total } => {
            let frac = if *total == 0 {
                0.0
            } else {
                *done as f32 / *total as f32
            };
            let scale = format!("transform: scaleX({frac:.3})");
            let count = format!("{done} / {total}");
            rsx! {
                div { class: "stage", role: "status",
                    div { class: "bar",
                        div { class: "bar-fill", style: "{scale}" }
                    }
                    p { class: "mono count", "{count}" }
                    p { class: "quiet", "Reading photos…" }
                }
            }
        }
        Phase::Grouping => rsx! {
            div { class: "stage", role: "status",
                div { class: "bar indeterminate" }
                p { class: "quiet", "Grouping variants…" }
            }
        },
        Phase::Error(msg) => {
            let msg = msg.clone();
            rsx! {
                div { class: "stage",
                    h2 { "Couldn’t read that folder" }
                    p { class: "error mono", "{msg}" }
                    button { class: "btn primary", onclick: pick_folder, "Try another folder…" }
                }
            }
        }
        Phase::Ready(s) if s.arrangement.groups.is_empty() => {
            let folder = s.folder.display().to_string();
            let failure_note = failure_summary(s.failures.len());
            rsx! {
                div { class: "stage",
                    h2 { "No photos here" }
                    p { class: "lede",
                        "Nothing in "
                        span { class: "mono", "{folder}" }
                        " looks like a photo. Sequin reads JPEG and PNG files."
                    }
                    if !s.failures.is_empty() {
                        p { class: "error mono", "{failure_note}" }
                    }
                    button { class: "btn primary", onclick: pick_folder, "Open another folder…" }
                }
            }
        }
        Phase::Ready(s) => {
            let failures: Vec<(String, String)> = s
                .failures
                .iter()
                .map(|(p, e)| (file_name(p), e.clone()))
                .collect();
            let failure_note = failure_summary(failures.len());
            let groups: Vec<GroupView> = s
                .arrangement
                .groups
                .iter()
                .enumerate()
                .map(|(gi, group)| GroupView {
                    key: group.photos[0].path.display().to_string(),
                    index: format!("{}", gi + 1),
                    stagger: format!("--i: {}", gi.min(14)),
                    meta: group_meta(group, &s.is_bw),
                    photos: group
                        .photos
                        .iter()
                        .map(|photo| PhotoView {
                            key: photo.path.display().to_string(),
                            src: format!("{}/{}", s.thumb_base, thumb_url_name(&photo.path)),
                            name: file_name(&photo.path),
                            aspect: format!(
                                "aspect-ratio: {} / {}",
                                photo.width.max(1),
                                photo.height.max(1)
                            ),
                        })
                        .collect(),
                })
                .collect();
            rsx! {
                if !failures.is_empty() {
                    details { class: "failures",
                        summary { "{failure_note}" }
                        ul {
                            for (name, err) in failures.iter() {
                                li { key: "{name}",
                                    span { class: "mono", "{name}" }
                                    " — {err}"
                                }
                            }
                        }
                    }
                }
                div { class: "groups",
                    for g in groups.iter() {
                        section { class: "group", key: "{g.key}", style: "{g.stagger}",
                            header { class: "group-head",
                                span { class: "g-index mono", "{g.index}" }
                                span { class: "g-meta mono", "{g.meta}" }
                            }
                            div { class: "g-row",
                                for p in g.photos.iter() {
                                    img {
                                        class: "thumb",
                                        key: "{p.key}",
                                        src: "{p.src}",
                                        alt: "Photo {p.name}",
                                        title: "{p.name}",
                                        loading: "lazy",
                                        decoding: "async",
                                        style: "{p.aspect}",
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        style { {include_str!("style.css")} }
        header { id: "app-head",
            h1 { "Sequin" }
            if let Some(text) = summary {
                p { class: "summary mono", "{text}" }
            }
            div { class: "spacer" }
            button { class: "btn", disabled: busy, onclick: pick_folder, "Open photo folder…" }
        }
        main { {content} }
    }
}

/// Kick off the scan → cluster pipeline for one folder, streaming progress.
fn start_scan(mut phase: Signal<Phase>, dir: PathBuf) {
    let cache_dir = thumbs::cache_dir_for(&cache_root(), &dir);
    let thumb_base = format!("/thumbs/{}", file_name(&cache_dir));

    spawn(async move {
        phase.set(Phase::Scanning { done: 0, total: 0 });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Prog>();

        let mut progress_phase = phase;
        spawn(async move {
            while let Some(p) = rx.recv().await {
                // Queued progress can arrive after the scan task has already
                // set Ready/Error; never regress a terminal phase.
                if !matches!(
                    *progress_phase.peek(),
                    Phase::Scanning { .. } | Phase::Grouping
                ) {
                    continue;
                }
                match p {
                    Prog::Scan(done, total) => progress_phase.set(Phase::Scanning { done, total }),
                    Prog::Grouping => progress_phase.set(Phase::Grouping),
                }
            }
        });

        let scan_dir = dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            let report = thumbs::scan_dir_with_thumbs(&scan_dir, &cache_dir, &|done, total| {
                let _ = tx.send(Prog::Scan(done, total));
            })?;
            let _ = tx.send(Prog::Grouping);
            let photos: Vec<_> = report.photos.iter().map(|s| s.photo.clone()).collect();
            let groups = grouping::cluster(&photos, grouping::DEFAULT_THRESHOLD)?;
            anyhow::Ok((Arrangement { groups }, report))
        })
        .await;

        match result {
            Ok(Ok((arrangement, report))) => {
                let is_bw = report
                    .photos
                    .iter()
                    .map(|s| (s.photo.path.clone(), s.is_bw))
                    .collect();
                phase.set(Phase::Ready(Session {
                    folder: dir,
                    arrangement,
                    thumb_base,
                    is_bw,
                    failures: report.failures,
                }));
            }
            Ok(Err(e)) => phase.set(Phase::Error(format!("{e:#}"))),
            Err(e) => phase.set(Phase::Error(format!("background task failed: {e}"))),
        }
    });
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn failure_summary(n: usize) -> String {
    plural(n, "file couldn’t be read", "files couldn’t be read")
}

/// `count photos · b&w · bordered` — the group badge line.
fn group_meta(group: &sequin_core::Group, is_bw: &HashMap<PathBuf, bool>) -> String {
    let mut meta = plural(group.photos.len(), "photo", "photos");
    if group
        .photos
        .iter()
        .any(|p| is_bw.get(&p.path).copied().unwrap_or(false))
    {
        meta.push_str(" · b&w");
    }
    if group.photos.iter().any(|p| p.border_fraction > 0.05) {
        meta.push_str(" · bordered");
    }
    meta
}

/// URL path segment for a photo's thumbnail (`<file name>.jpg`, encoded).
fn thumb_url_name(photo_path: &std::path::Path) -> String {
    utf8_percent_encode(&format!("{}.jpg", file_name(photo_path)), URL_ESCAPED).to_string()
}
