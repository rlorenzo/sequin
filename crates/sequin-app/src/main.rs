//! Sequin desktop app (Dioxus).
//!
//! M2: grouped thumbnail grid; M3: arrangement editing — drag/keyboard
//! reordering of groups and photos, selection, merge/split, undo, and an
//! autosaved `arrangement.json` sidecar that doubles as session resume and
//! CLI interchange. See PLAN.md M4 for the time-assignment + EXIF-write flow.

use dioxus::desktop::wry::http::Response;
use dioxus::desktop::{use_asset_handler, Config, WindowBuilder};
use dioxus::prelude::*;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sequin_core::{arrange, grouping, thumbs, Arrangement};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

/// Escape everything but RFC 3986 unreserved characters in thumbnail URLs.
const URL_ESCAPED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

const UNDO_CAP: usize = 100;

/// Autosave generation: bumped on every save and on every new scan, so a
/// stale queued write can neither run nor stamp a newer session's header.
static SAVE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use std::sync::atomic::Ordering;

fn main() {
    let window = WindowBuilder::new()
        .with_title("Sequin")
        .with_inner_size(dioxus::desktop::LogicalSize::new(1060.0, 760.0))
        .with_min_inner_size(dioxus::desktop::LogicalSize::new(680.0, 480.0));
    dioxus::LaunchBuilder::new()
        .with_cfg(Config::new().with_window(window))
        .launch(app);
}

/// Everything the editable light table needs about a completed scan.
#[derive(Clone)]
struct Session {
    folder: PathBuf,
    arrangement: Arrangement,
    /// URL prefix thumbnails are served under: `/thumbs/<cache-key>`.
    thumb_base: String,
    is_bw: HashMap<PathBuf, bool>,
    failures: Vec<(PathBuf, String)>,
    /// True when the arrangement came from a saved sidecar, not clustering.
    resumed: bool,
}

/// Scan lifecycle; the session itself lives in its own signal so edits don't
/// touch the phase machinery.
#[derive(Clone, PartialEq)]
enum Phase {
    Idle,
    Scanning { done: usize, total: usize },
    Grouping,
    Ready,
    Error(String),
}

/// Progress messages sent from the blocking scan into the UI.
enum Prog {
    Scan(usize, usize),
    Grouping,
}

/// A selectable / draggable thing, addressed by current indices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sel {
    Photo(usize, usize),
    Group(usize),
}

/// Where a drag is currently hovering.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hover {
    /// Insert before photo `.1` of group `.0` (`usize::MAX` appends).
    Slot(usize, usize),
    /// The gap before group index `.0` (`len` is the end).
    Gap(usize),
    /// Onto a group as a whole (merge target for group drags).
    OnGroup(usize),
}

/// Ambient persistence state shown in the header.
#[derive(Clone, PartialEq)]
enum SaveState {
    /// No edits yet this session — nothing to say.
    Untouched,
    Saved,
    Failed(String),
}

/// One undo step: the arrangement and the selection that went with it.
type Snapshot = (Arrangement, Vec<Sel>);

/// All editing state bundled so handlers thread one struct of `Copy` signals.
#[derive(Clone, Copy, PartialEq)]
struct Editor {
    session: Signal<Option<Session>>,
    selection: Signal<Vec<Sel>>,
    drag: Signal<Option<Sel>>,
    hover: Signal<Option<Hover>>,
    undo: Signal<Vec<Snapshot>>,
    redo: Signal<Vec<Snapshot>>,
    save_state: Signal<SaveState>,
    /// Screen-reader announcement, rendered into a polite live region.
    announce: Signal<String>,
}

impl Editor {
    /// Run one mutation: snapshot for undo, apply, reselect, announce,
    /// autosave.
    fn apply(&mut self, select_after: Option<Sel>, said: &str, f: impl FnOnce(&mut Arrangement)) {
        let (folder, arr) = {
            let mut w = self.session.write();
            let Some(s) = w.as_mut() else { return };
            let before = s.arrangement.clone();
            f(&mut s.arrangement);
            if s.arrangement.groups == before.groups {
                return; // no-op edit: no undo entry, no save
            }
            let mut undo = self.undo.write();
            if undo.len() >= UNDO_CAP {
                undo.remove(0);
            }
            undo.push((before, self.selection.read().clone()));
            self.redo.write().clear();
            (s.folder.clone(), s.arrangement.clone())
        };
        if let Some(sel) = select_after {
            self.selection.set(vec![sel]);
        }
        self.announce.set(said.to_string());
        self.autosave(folder, arr);
    }

    fn undo_edit(&mut self) {
        self.time_travel(true);
    }

    fn redo_edit(&mut self) {
        self.time_travel(false);
    }

    fn time_travel(&mut self, back: bool) {
        let (folder, arr, restore_sel) = {
            let mut w = self.session.write();
            let Some(s) = w.as_mut() else { return };
            let (mut from, mut to) = (self.undo, self.redo);
            if !back {
                std::mem::swap(&mut from, &mut to);
            }
            let Some((prev_arr, prev_sel)) = from.write().pop() else {
                self.announce.set(
                    if back {
                        "Nothing to undo"
                    } else {
                        "Nothing to redo"
                    }
                    .to_string(),
                );
                return;
            };
            to.write()
                .push((s.arrangement.clone(), self.selection.read().clone()));
            s.arrangement = prev_arr;
            (s.folder.clone(), s.arrangement.clone(), prev_sel)
        };
        self.selection.set(restore_sel);
        self.announce
            .set(if back { "Undid edit" } else { "Redid edit" }.to_string());
        self.autosave(folder, arr);
    }

    /// Fire-and-forget sidecar write; state surfaces quietly in the header.
    /// Rapid edits overlap saves, so writes are serialized behind a lock and
    /// a save superseded while queued skips writing — the newest arrangement
    /// always lands on disk last and owns the header state.
    fn autosave(&self, folder: PathBuf, arr: Arrangement) {
        static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let my_gen = SAVE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        let mut save_state = self.save_state;
        spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
                (SAVE_GEN.load(Ordering::SeqCst) == my_gen).then(|| arrange::save(&arr, &folder))
            })
            .await;
            // A save that finished after being superseded (newer edit, or a
            // new session bumped the generation) must not touch the header.
            if SAVE_GEN.load(Ordering::SeqCst) != my_gen {
                return;
            }
            match result {
                Ok(None) => {} // superseded by a newer save while queued
                Ok(Some(Ok(()))) => save_state.set(SaveState::Saved),
                Ok(Some(Err(e))) => save_state.set(SaveState::Failed(format!("{e:#}"))),
                Err(e) => save_state.set(SaveState::Failed(format!("save task failed: {e}"))),
            }
        });
    }

    fn select(&mut self, items: Vec<Sel>, said: String) {
        self.selection.set(items);
        self.announce.set(said);
    }
}

fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("sequin")
        .join("thumbs")
}

fn app() -> Element {
    let phase = use_signal(|| Phase::Idle);
    let editor = Editor {
        session: use_signal(|| None),
        selection: use_signal(Vec::new),
        drag: use_signal(|| None),
        hover: use_signal(|| None),
        undo: use_signal(Vec::new),
        redo: use_signal(Vec::new),
        save_state: use_signal(|| SaveState::Untouched),
        announce: use_signal(String::new),
    };

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
            start_scan(phase, editor, PathBuf::from(dir));
        }
    });

    let pick_folder = move |_| {
        spawn(async move {
            if let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await {
                start_scan(phase, editor, dir.path().to_path_buf());
            }
        });
    };

    let current = phase.read().clone();
    let busy = matches!(current, Phase::Scanning { .. } | Phase::Grouping);
    let summary = editor.session.read().as_ref().and_then(|s| {
        (!s.arrangement.groups.is_empty()).then(|| {
            format!(
                "{} · {}",
                plural(s.arrangement.photo_count(), "photo", "photos"),
                plural(s.arrangement.groups.len(), "group", "groups")
            )
        })
    });
    let save_state = editor.save_state.read().clone();

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
        Phase::Ready => rsx! {
            LightTable { editor, pick_folder }
        },
    };

    rsx! {
        style { {include_str!("style.css")} }
        header { id: "app-head",
            h1 { "Sequin" }
            if let Some(text) = summary {
                p { class: "summary mono", "{text}" }
            }
            match save_state {
                SaveState::Untouched => rsx! {},
                SaveState::Saved => rsx! {
                    p { class: "save-ok mono", "saved" }
                },
                SaveState::Failed(err) => rsx! {
                    p { class: "save-err mono", title: "{err}", "couldn’t save arrangement" }
                },
            }
            div { class: "spacer" }
            button { class: "btn", disabled: busy, onclick: pick_folder, "Open photo folder…" }
        }
        main { {content} }
    }
}

/// The editable grid: groups as rows, gaps as drop zones, full keyboard map.
#[component]
fn LightTable(editor: Editor, pick_folder: EventHandler<MouseEvent>) -> Element {
    let mut ed = editor;
    let session = editor.session.read();
    let Some(s) = session.as_ref() else {
        return rsx! {
            div { class: "stage",
                p { class: "quiet", "No session." }
            }
        };
    };

    if s.arrangement.groups.is_empty() {
        let folder = s.folder.display().to_string();
        let failure_note = failure_summary(s.failures.len());
        let has_failures = !s.failures.is_empty();
        return rsx! {
            div { class: "stage",
                h2 { "No photos here" }
                p { class: "lede",
                    "Nothing in "
                    span { class: "mono", "{folder}" }
                    " looks like a photo. Sequin reads JPEG and PNG files."
                }
                if has_failures {
                    p { class: "error mono", "{failure_note}" }
                }
                button { class: "btn primary", onclick: move |e| pick_folder.call(e), "Open another folder…" }
            }
        };
    }

    let selection = editor.selection.read().clone();
    let hover = *editor.hover.read();
    let dragging = *editor.drag.read();
    let groups = build_views(s, &selection, hover, dragging);
    let gap_count = s.arrangement.groups.len() + 1;
    let end_gap_active = hover == Some(Hover::Gap(gap_count - 1));
    let failures: Vec<(String, String)> = s
        .failures
        .iter()
        .map(|(p, e)| (file_name(p), e.clone()))
        .collect();
    let failure_note = failure_summary(failures.len());
    let resumed = s.resumed;
    drop(session);

    let announce = editor.announce.read().clone();
    // Keyboard cursor for assistive tech: the last-selected element's id.
    let active_id = selection
        .last()
        .map(|sel| match sel {
            Sel::Photo(gi, pi) => format!("cell-{gi}-{pi}"),
            Sel::Group(gi) => format!("grp-{gi}"),
        })
        .unwrap_or_default();

    let on_key = move |evt: KeyboardEvent| handle_key(&mut ed, evt);

    rsx! {
        div {
            class: "table",
            tabindex: "0",
            role: "application",
            aria_label: "Photo arrangement editor. Arrow keys move selection, command with arrows moves the selected photo or group, M merges, S splits, command Z undoes.",
            aria_activedescendant: "{active_id}",
            onkeydown: on_key,
            onmounted: move |evt| {
                spawn(async move {
                    let _ = evt.data().set_focus(true).await;
                });
            },
            onclick: move |_| ed.select(Vec::new(), "Selection cleared".into()),

            div { class: "sr-only", aria_live: "polite", "{announce}" }

            if resumed {
                p { class: "resumed mono",
                    "Resumed your saved arrangement — reopen anytime to continue."
                }
            }
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
                for g in groups.into_iter() {
                    Fragment { key: "{g.key}",
                        GapZone { editor, index: g.gi, active: g.gap_above_active }
                        GroupRow { editor, view: g }
                    }
                }
                GapZone { editor, index: gap_count - 1, active: end_gap_active }
            }
            p { class: "hints mono",
                "drag or ⌘arrows reorder · arrows navigate · ⇧/⌘click select more · "
                "M merge · S split · ⌘Z undo · ⇧⌘Z redo · esc clear"
            }
        }
    }
}

/// One gap between groups: a drop zone for group reorders and photo split-outs.
#[component]
fn GapZone(editor: Editor, index: usize, active: bool) -> Element {
    let mut ed = editor;
    let class = if active { "gap active" } else { "gap" };
    rsx! {
        div {
            class: "{class}",
            ondragover: move |evt| {
                evt.prevent_default();
                ed.hover.set(Some(Hover::Gap(index)));
            },
            ondragleave: move |_| ed.hover.set(None),
            ondrop: move |evt| {
                evt.prevent_default();
                drop_on(&mut ed, Hover::Gap(index));
            },
        }
    }
}

/// Per-group view model, precomputed so the RSX stays plain.
#[derive(Clone, PartialEq)]
struct GroupView {
    key: String,
    gi: usize,
    index_label: String,
    meta: String,
    selected: bool,
    drag_source: bool,
    merge_target: bool,
    append_active: bool,
    gap_above_active: bool,
    photos: Vec<PhotoView>,
}

#[derive(Clone, PartialEq)]
struct PhotoView {
    key: String,
    gi: usize,
    pi: usize,
    src: String,
    name: String,
    aspect: String,
    selected: bool,
    drag_source: bool,
    insert_before: bool,
}

#[component]
fn GroupRow(editor: Editor, view: GroupView) -> Element {
    let mut ed = editor;
    let gi = view.gi;
    let mut cls = String::from("group");
    if view.selected {
        cls.push_str(" selected");
    }
    if view.drag_source {
        cls.push_str(" drag-source");
    }
    if view.merge_target {
        cls.push_str(" merge-target");
    }
    let end_class = if view.append_active {
        "cell-end active"
    } else {
        "cell-end"
    };

    rsx! {
        section { class: "{cls}",
            header {
                class: "group-head",
                draggable: "true",
                ondragstart: move |_| {
                    ed.drag.set(Some(Sel::Group(gi)));
                    ed.selection.set(vec![Sel::Group(gi)]);
                },
                ondragend: move |_| {
                    ed.drag.set(None);
                    ed.hover.set(None);
                },
                ondragover: move |evt| {
                    evt.prevent_default();
                    let target = head_target(&ed, gi);
                    ed.hover.set(Some(target));
                },
                ondrop: move |evt| {
                    evt.prevent_default();
                    let target = head_target(&ed, gi);
                    drop_on(&mut ed, target);
                },
                button {
                    class: "g-index mono",
                    id: "grp-{gi}",
                    aria_pressed: view.selected,
                    aria_label: "Group {view.index_label}, {view.meta}",
                    onclick: move |evt: MouseEvent| {
                        evt.stop_propagation();
                        select_click(
                            &mut ed,
                            Sel::Group(gi),
                            evt.modifiers().contains(Modifiers::META),
                            evt.modifiers().contains(Modifiers::SHIFT),
                        );
                    },
                    "{view.index_label}"
                }
                span { class: "g-meta mono", "{view.meta}" }
            }
            div { class: "g-row",
                for p in view.photos.into_iter() {
                    PhotoCell { key: "{p.key}", editor, view: p }
                }
                div {
                    class: "{end_class}",
                    // Same rule as PhotoCell: photo rows accept photo drags
                    // only, so a group drag never lights a drop it ignores.
                    ondragover: move |evt| {
                        if !matches!(*ed.drag.read(), Some(Sel::Group(_))) {
                            evt.prevent_default();
                            ed.hover.set(Some(Hover::Slot(gi, usize::MAX)));
                        }
                    },
                    ondragleave: move |_| ed.hover.set(None),
                    ondrop: move |evt| {
                        if !matches!(*ed.drag.read(), Some(Sel::Group(_))) {
                            evt.prevent_default();
                            drop_on(&mut ed, Hover::Slot(gi, usize::MAX));
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn PhotoCell(editor: Editor, view: PhotoView) -> Element {
    let mut ed = editor;
    let (gi, pi) = (view.gi, view.pi);
    let mut cls = String::from("cell");
    if view.selected {
        cls.push_str(" selected");
    }
    if view.drag_source {
        cls.push_str(" drag-source");
    }
    if view.insert_before {
        cls.push_str(" insert-before");
    }

    rsx! {
        div {
            class: "{cls}",
            id: "cell-{gi}-{pi}",
            draggable: "true",
            ondragstart: move |_| {
                ed.drag.set(Some(Sel::Photo(gi, pi)));
                ed.selection.set(vec![Sel::Photo(gi, pi)]);
            },
            ondragend: move |_| {
                ed.drag.set(None);
                ed.hover.set(None);
            },
            // Photo rows accept photo drags only; group drags must land on a
            // group head (merge) or a gap (reorder) — the whole-body merge
            // target was an accidental-merge trap.
            ondragover: move |evt| {
                if !matches!(*ed.drag.read(), Some(Sel::Group(_))) {
                    evt.prevent_default();
                    ed.hover.set(Some(Hover::Slot(gi, pi)));
                }
            },
            ondrop: move |evt| {
                if !matches!(*ed.drag.read(), Some(Sel::Group(_))) {
                    evt.prevent_default();
                    drop_on(&mut ed, Hover::Slot(gi, pi));
                }
            },
            onclick: move |evt: MouseEvent| {
                evt.stop_propagation();
                select_click(
                    &mut ed,
                    Sel::Photo(gi, pi),
                    evt.modifiers().contains(Modifiers::META),
                    evt.modifiers().contains(Modifiers::SHIFT),
                );
            },
            img {
                class: "thumb",
                src: "{view.src}",
                alt: "Photo {view.name}",
                title: "{view.name}",
                loading: "lazy",
                decoding: "async",
                style: "{view.aspect}",
            }
        }
    }
}

fn build_views(
    s: &Session,
    selection: &[Sel],
    hover: Option<Hover>,
    dragging: Option<Sel>,
) -> Vec<GroupView> {
    s.arrangement
        .groups
        .iter()
        .enumerate()
        .map(|(gi, group)| GroupView {
            key: group.photos[0].path.display().to_string(),
            gi,
            index_label: format!("{}", gi + 1),
            meta: group_meta(group, &s.is_bw),
            selected: selection.contains(&Sel::Group(gi)),
            drag_source: dragging == Some(Sel::Group(gi)),
            merge_target: hover == Some(Hover::OnGroup(gi)),
            append_active: hover == Some(Hover::Slot(gi, usize::MAX)),
            gap_above_active: hover == Some(Hover::Gap(gi)),
            photos: group
                .photos
                .iter()
                .enumerate()
                .map(|(pi, photo)| PhotoView {
                    key: photo.path.display().to_string(),
                    gi,
                    pi,
                    src: format!("{}/{}", s.thumb_base, thumb_url_name(&photo.path)),
                    name: file_name(&photo.path),
                    aspect: format!(
                        "aspect-ratio: {} / {}",
                        photo.width.max(1),
                        photo.height.max(1)
                    ),
                    selected: selection.contains(&Sel::Photo(gi, pi)),
                    drag_source: dragging == Some(Sel::Photo(gi, pi)),
                    insert_before: hover == Some(Hover::Slot(gi, pi)),
                })
                .collect(),
        })
        .collect()
}

/// What a drag hovering over a group head means: group drags merge onto the
/// group; photo drags append to its row.
fn head_target(ed: &Editor, gi: usize) -> Hover {
    match *ed.drag.read() {
        Some(Sel::Group(_)) => Hover::OnGroup(gi),
        _ => Hover::Slot(gi, usize::MAX),
    }
}

/// Execute the drop of the current drag payload onto `target`.
fn drop_on(ed: &mut Editor, target: Hover) {
    let Some(payload) = ed.drag.write().take() else {
        ed.hover.set(None);
        return;
    };
    ed.hover.set(None);

    let (select_path, singleton) = {
        let session = ed.session.read();
        let Some(s) = session.as_ref() else { return };
        let arr = &s.arrangement;
        let select_path = match payload {
            Sel::Photo(fg, fp) => arr
                .groups
                .get(fg)
                .and_then(|g| g.photos.get(fp))
                .map(|p| p.path.clone()),
            Sel::Group(fg) => arr
                .groups
                .get(fg)
                .and_then(|g| g.photos.first())
                .map(|p| p.path.clone()),
        };
        let singleton = matches!(
            payload,
            Sel::Photo(fg, _) if arr.groups.get(fg).map(|g| g.photos.len()) == Some(1)
        );
        (select_path, singleton)
    };

    match (payload, target) {
        (Sel::Photo(fg, fp), Hover::Slot(tg, tp)) => {
            let said = if tg == fg {
                format!("Moved photo within group {}", tg + 1)
            } else {
                format!("Moved photo to group {}", tg + 1)
            };
            ed.apply(None, &said, |arr| {
                let tp = if tp == usize::MAX {
                    arr.groups.get(tg).map(|g| g.photos.len()).unwrap_or(0)
                } else {
                    tp
                };
                arrange::move_photo(arr, (fg, fp), tg, tp);
            });
        }
        (Sel::Photo(fg, fp), Hover::Gap(g)) if !singleton => {
            ed.apply(None, "Split photo into its own group", |arr| {
                arrange::split_photo(arr, (fg, fp), g)
            });
        }
        // A group — or a lone photo, which IS its group — into a gap reorders.
        (Sel::Photo(fg, _), Hover::Gap(g)) | (Sel::Group(fg), Hover::Gap(g)) => {
            let to = if g > fg { g - 1 } else { g };
            let said = format!("Moved group {} to position {}", fg + 1, to + 1);
            ed.apply(None, &said, |arr| arrange::move_group(arr, fg, to));
        }
        (Sel::Group(fg), Hover::OnGroup(tg)) => {
            let said = format!("Merged group {} into group {}", fg + 1, tg + 1);
            ed.apply(None, &said, |arr| arrange::merge_groups(arr, tg, fg));
        }
        (Sel::Photo(..), Hover::OnGroup(_)) | (Sel::Group(..), Hover::Slot(..)) => {}
    }

    // Reselect the moved thing at its new location.
    if let Some(path) = select_path {
        reselect_by_path(ed, &path, matches!(payload, Sel::Group(_)));
    }
}

fn reselect_by_path(ed: &mut Editor, path: &std::path::Path, as_group: bool) {
    let found = {
        let session = ed.session.read();
        let Some(s) = session.as_ref() else { return };
        arrange::find_photo(&s.arrangement, path)
    };
    if let Some((gi, pi)) = found {
        let sel = if as_group {
            Sel::Group(gi)
        } else {
            Sel::Photo(gi, pi)
        };
        ed.selection.set(vec![sel]);
    }
}

fn describe(item: Sel) -> String {
    match item {
        Sel::Photo(gi, pi) => format!("photo {} of group {}", pi + 1, gi + 1),
        Sel::Group(gi) => format!("group {}", gi + 1),
    }
}

/// Click selection: plain replaces, cmd toggles, shift extends a range from
/// the current cursor (photos flatten across groups; groups span indices).
fn select_click(ed: &mut Editor, item: Sel, meta: bool, shift: bool) {
    let cursor = ed.selection.read().last().copied();
    if shift {
        if let Some(range) = selection_range(ed, cursor, item) {
            let n = range.len();
            ed.select(range, format!("Selected {n} items"));
            return;
        }
    }
    if meta {
        let mut sel = ed.selection.read().clone();
        let said = if let Some(pos) = sel.iter().position(|s| *s == item) {
            sel.remove(pos);
            format!("Removed {} from selection", describe(item))
        } else {
            sel.push(item);
            format!(
                "Added {} to selection, {} selected",
                describe(item),
                sel.len()
            )
        };
        ed.select(sel, said);
    } else {
        ed.select(vec![item], format!("Selected {}", describe(item)));
    }
}

/// Inclusive range between cursor and target when both are the same kind.
fn selection_range(ed: &Editor, cursor: Option<Sel>, to: Sel) -> Option<Vec<Sel>> {
    let session = ed.session.read();
    let s = session.as_ref()?;
    match (cursor?, to) {
        (Sel::Group(a), Sel::Group(b)) => {
            let (lo, hi) = (a.min(b), a.max(b));
            Some((lo..=hi).map(Sel::Group).collect())
        }
        (Sel::Photo(g0, p0), Sel::Photo(g1, p1)) => {
            let flat: Vec<Sel> = s
                .arrangement
                .groups
                .iter()
                .enumerate()
                .flat_map(|(gi, g)| (0..g.photos.len()).map(move |pi| Sel::Photo(gi, pi)))
                .collect();
            let a = flat.iter().position(|s| *s == Sel::Photo(g0, p0))?;
            let b = flat.iter().position(|s| *s == Sel::Photo(g1, p1))?;
            let (lo, hi) = (a.min(b), a.max(b));
            Some(flat[lo..=hi].to_vec())
        }
        _ => None,
    }
}

fn handle_key(ed: &mut Editor, evt: KeyboardEvent) {
    let meta = evt.modifiers().contains(Modifiers::META);
    let shift = evt.modifiers().contains(Modifiers::SHIFT);
    let key = evt.key();

    // Current cursor: the last selected item.
    let cursor = ed.selection.read().last().copied();
    let (group_count, photo_counts): (usize, Vec<usize>) = {
        let session = ed.session.read();
        let Some(s) = session.as_ref() else { return };
        (
            s.arrangement.groups.len(),
            s.arrangement
                .groups
                .iter()
                .map(|g| g.photos.len())
                .collect(),
        )
    };

    match key {
        Key::Escape => {
            ed.select(Vec::new(), "Selection cleared".into());
        }
        Key::Character(c) if meta && (c == "z" || c == "Z") => {
            evt.prevent_default();
            if shift {
                ed.redo_edit();
            } else {
                ed.undo_edit();
            }
        }
        Key::Character(c) if !meta && (c == "m" || c == "M") => {
            evt.prevent_default();
            merge_selected_groups(ed);
        }
        Key::Character(c) if !meta && (c == "s" || c == "S") => {
            evt.prevent_default();
            split_selected_photos(ed);
        }
        Key::ArrowLeft | Key::ArrowRight if !meta => {
            evt.prevent_default();
            let forward = key == Key::ArrowRight;
            let next = match cursor {
                Some(Sel::Photo(gi, pi)) => step_photo(&photo_counts, gi, pi, forward),
                Some(Sel::Group(gi)) => Some(Sel::Photo(gi, 0)),
                None => (group_count > 0).then_some(Sel::Photo(0, 0)),
            };
            if let Some(n) = next {
                extend_or_replace(ed, n, shift);
            }
        }
        Key::ArrowUp | Key::ArrowDown if !meta => {
            evt.prevent_default();
            let down = key == Key::ArrowDown;
            let current_group = match cursor {
                Some(Sel::Photo(gi, _)) | Some(Sel::Group(gi)) => Some(gi),
                None => None,
            };
            let next = match current_group {
                Some(gi) if down && gi + 1 < group_count => Some(Sel::Group(gi + 1)),
                Some(gi) if !down && gi > 0 => Some(Sel::Group(gi - 1)),
                Some(gi) => Some(Sel::Group(gi)),
                None => (group_count > 0).then_some(Sel::Group(0)),
            };
            if let Some(n) = next {
                extend_or_replace(ed, n, shift);
            }
        }
        Key::ArrowLeft | Key::ArrowRight if meta => {
            evt.prevent_default();
            if let Some(Sel::Photo(gi, pi)) = cursor {
                let right = key == Key::ArrowRight;
                let len = photo_counts.get(gi).copied().unwrap_or(0);
                if right && pi + 1 < len {
                    let said = format!("Moved photo to position {} in group {}", pi + 2, gi + 1);
                    ed.apply(Some(Sel::Photo(gi, pi + 1)), &said, |arr| {
                        arrange::move_photo(arr, (gi, pi), gi, pi + 2)
                    });
                } else if !right && pi > 0 {
                    let said = format!("Moved photo to position {} in group {}", pi, gi + 1);
                    ed.apply(Some(Sel::Photo(gi, pi - 1)), &said, |arr| {
                        arrange::move_photo(arr, (gi, pi), gi, pi - 1)
                    });
                }
            }
        }
        Key::ArrowUp | Key::ArrowDown if meta => {
            evt.prevent_default();
            let down = key == Key::ArrowDown;
            match cursor {
                Some(Sel::Group(gi)) => {
                    if down && gi + 1 < group_count {
                        let said = format!("Moved group to position {}", gi + 2);
                        ed.apply(Some(Sel::Group(gi + 1)), &said, |arr| {
                            arrange::move_group(arr, gi, gi + 1)
                        });
                    } else if !down && gi > 0 {
                        let said = format!("Moved group to position {}", gi);
                        ed.apply(Some(Sel::Group(gi - 1)), &said, |arr| {
                            arrange::move_group(arr, gi, gi - 1)
                        });
                    }
                }
                Some(Sel::Photo(gi, pi)) => {
                    // Move the photo into the neighboring group (append).
                    let target = if down {
                        (gi + 1 < group_count).then_some(gi + 1)
                    } else {
                        gi.checked_sub(1)
                    };
                    if let Some(tg) = target {
                        let path = photo_path(ed, gi, pi);
                        let said = format!("Moved photo to group {}", tg + 1);
                        ed.apply(None, &said, |arr| {
                            let tp = arr.groups.get(tg).map(|g| g.photos.len()).unwrap_or(0);
                            arrange::move_photo(arr, (gi, pi), tg, tp);
                        });
                        if let Some(p) = path {
                            reselect_by_path(ed, &p, false);
                        }
                    }
                }
                None => {}
            }
        }
        _ => {}
    }
}

/// Arrow-key selection: shift extends (adds the next item, cursor moves to
/// it), plain replaces. Extension is what makes keyboard-only merge possible.
fn extend_or_replace(ed: &mut Editor, next: Sel, shift: bool) {
    if shift {
        let mut sel = ed.selection.read().clone();
        // Re-pushing an already-selected item just makes it the cursor.
        sel.retain(|s| *s != next);
        sel.push(next);
        let n = sel.len();
        ed.select(
            sel,
            format!("Added {} to selection, {n} selected", describe(next)),
        );
    } else {
        ed.select(vec![next], format!("Selected {}", describe(next)));
    }
}

fn photo_path(ed: &Editor, gi: usize, pi: usize) -> Option<PathBuf> {
    ed.session
        .read()
        .as_ref()
        .and_then(|s| s.arrangement.groups.get(gi))
        .and_then(|g| g.photos.get(pi))
        .map(|p| p.path.clone())
}

fn step_photo(counts: &[usize], gi: usize, pi: usize, forward: bool) -> Option<Sel> {
    if forward {
        if pi + 1 < counts[gi] {
            Some(Sel::Photo(gi, pi + 1))
        } else if gi + 1 < counts.len() {
            Some(Sel::Photo(gi + 1, 0))
        } else {
            None
        }
    } else if pi > 0 {
        Some(Sel::Photo(gi, pi - 1))
    } else if gi > 0 {
        // checked_sub: empty groups shouldn't exist, but never underflow.
        counts[gi - 1]
            .checked_sub(1)
            .map(|last| Sel::Photo(gi - 1, last))
    } else {
        None
    }
}

/// Merge every selected group into the first-selected one. With exactly one
/// group selected, merges the group below into it — the single-key keyboard
/// path.
fn merge_selected_groups(ed: &mut Editor) {
    let mut groups: Vec<usize> = ed
        .selection
        .read()
        .iter()
        .filter_map(|s| match s {
            Sel::Group(gi) => Some(*gi),
            _ => None,
        })
        .collect();
    if groups.len() == 1 {
        let below = groups[0] + 1;
        let has_below = ed
            .session
            .read()
            .as_ref()
            .is_some_and(|s| below < s.arrangement.groups.len());
        if !has_below {
            ed.announce.set("No group below to merge with".into());
            return;
        }
        groups.push(below);
    }
    if groups.len() < 2 {
        ed.announce.set("Select two or more groups to merge".into());
        return;
    }
    // Identify by first-photo path so indices survive successive merges.
    let paths: Vec<PathBuf> = {
        let session = ed.session.read();
        let Some(s) = session.as_ref() else { return };
        groups
            .iter()
            .filter_map(|gi| s.arrangement.groups.get(*gi))
            .filter_map(|g| g.photos.first())
            .map(|p| p.path.clone())
            .collect()
    };
    let Some((target_path, sources)) = paths.split_first() else {
        return;
    };
    let target_path = target_path.clone();
    let sources = sources.to_vec();
    let reselect = target_path.clone();
    let said = format!("Merged {} groups", sources.len() + 1);
    ed.apply(None, &said, move |arr| {
        for source_path in &sources {
            let t = arrange::find_photo(arr, &target_path).map(|(gi, _)| gi);
            let sr = arrange::find_photo(arr, source_path).map(|(gi, _)| gi);
            if let (Some(t), Some(sr)) = (t, sr) {
                arrange::merge_groups(arr, t, sr);
            }
        }
    });
    reselect_by_path(ed, &reselect, true);
}

/// Split every selected photo out into its own group (after its source group).
fn split_selected_photos(ed: &mut Editor) {
    let photo_paths: Vec<PathBuf> = {
        let session = ed.session.read();
        let Some(s) = session.as_ref() else { return };
        ed.selection
            .read()
            .iter()
            .filter_map(|sel| match sel {
                Sel::Photo(gi, pi) => s
                    .arrangement
                    .groups
                    .get(*gi)
                    .and_then(|g| g.photos.get(*pi))
                    .map(|p| p.path.clone()),
                _ => None,
            })
            .collect()
    };
    if photo_paths.is_empty() {
        ed.announce.set("Select a photo to split out".into());
        return;
    }
    let said = plural(photo_paths.len(), "photo split out", "photos split out");
    let reselect = photo_paths.clone();
    ed.apply(None, &said, move |arr| {
        for path in &photo_paths {
            if let Some((gi, pi)) = arrange::find_photo(arr, path) {
                if arr.groups[gi].photos.len() > 1 {
                    arrange::split_photo(arr, (gi, pi), gi + 1);
                }
            }
        }
    });
    // Selection indices are stale after splitting; reselect every split
    // photo at its new location (mirrors multi-group merge behaviour).
    let sels: Vec<Sel> = {
        let session = ed.session.read();
        session
            .as_ref()
            .map(|s| {
                reselect
                    .iter()
                    .filter_map(|p| arrange::find_photo(&s.arrangement, p))
                    .map(|(gi, pi)| Sel::Photo(gi, pi))
                    .collect()
            })
            .unwrap_or_default()
    };
    if !sels.is_empty() {
        ed.selection.set(sels);
    }
}

/// Kick off the scan → cluster pipeline for one folder, streaming progress.
/// A valid saved sidecar covering the same photo set wins over re-clustering.
fn start_scan(mut phase: Signal<Phase>, mut editor: Editor, dir: PathBuf) {
    let cache_dir = thumbs::cache_dir_for(&cache_root(), &dir);
    let thumb_base = format!(
        "/thumbs/{}",
        cache_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );

    spawn(async move {
        editor.session.set(None);
        editor.selection.set(Vec::new());
        editor.undo.set(Vec::new());
        editor.redo.set(Vec::new());
        // Invalidate any queued autosave from the previous session before
        // resetting the header state it would otherwise stamp.
        SAVE_GEN.fetch_add(1, Ordering::SeqCst);
        editor.save_state.set(SaveState::Untouched);
        editor.announce.set(String::new());
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
            let mut report =
                thumbs::scan_dir_with_thumbs(&scan_dir, &cache_dir, &|done, total| {
                    let _ = tx.send(Prog::Scan(done, total));
                })?;
            let _ = tx.send(Prog::Grouping);
            let photos: Vec<_> = report.photos.iter().map(|s| s.photo.clone()).collect();
            // Resume a saved arrangement when it covers exactly these photos.
            let (arrangement, resumed) = match arrange::load(&scan_dir) {
                Ok(Some(saved)) if arrange::covers_same_photos(&saved, &photos) => (saved, true),
                Err(e) => {
                    // An unreadable sidecar may hold hours of manual ordering;
                    // move it aside before the first autosave overwrites it,
                    // and surface the error in the failures list.
                    let side = arrange::sidecar_path(&scan_dir);
                    let bad = side.with_extension("json.bad");
                    let note = match std::fs::rename(&side, &bad) {
                        Ok(()) => format!(
                            "saved arrangement couldn’t be read; moved aside to {} — {e:#}",
                            file_name(&bad)
                        ),
                        Err(re) => format!(
                            "saved arrangement couldn’t be read ({e:#}) or moved aside ({re})"
                        ),
                    };
                    report.failures.push((side, note));
                    let groups = grouping::cluster(&photos, grouping::DEFAULT_THRESHOLD)?;
                    (Arrangement { groups }, false)
                }
                _ => {
                    let groups = grouping::cluster(&photos, grouping::DEFAULT_THRESHOLD)?;
                    (Arrangement { groups }, false)
                }
            };
            anyhow::Ok((arrangement, resumed, report))
        })
        .await;

        match result {
            Ok(Ok((arrangement, resumed, report))) => {
                let is_bw = report
                    .photos
                    .iter()
                    .map(|s| (s.photo.path.clone(), s.is_bw))
                    .collect();
                editor.session.set(Some(Session {
                    folder: dir,
                    arrangement,
                    thumb_base,
                    is_bw,
                    failures: report.failures,
                    resumed,
                }));
                phase.set(Phase::Ready);
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
    let name = photo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    utf8_percent_encode(&format!("{name}.jpg"), URL_ESCAPED).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sequin_core::{Group, Photo};
    use std::path::Path;

    fn photo(name: &str, border: f32) -> Photo {
        Photo {
            path: PathBuf::from(format!("/d/{name}")),
            hash_full: String::new(),
            hash_cropped: String::new(),
            border_fraction: border,
            width: 100,
            height: 100,
        }
    }

    #[test]
    fn step_photo_walks_across_group_boundaries() {
        let counts = [2, 1];
        assert_eq!(step_photo(&counts, 0, 0, true), Some(Sel::Photo(0, 1)));
        assert_eq!(step_photo(&counts, 0, 1, true), Some(Sel::Photo(1, 0)));
        assert_eq!(step_photo(&counts, 1, 0, true), None);
        assert_eq!(step_photo(&counts, 1, 0, false), Some(Sel::Photo(0, 1)));
        assert_eq!(step_photo(&counts, 0, 1, false), Some(Sel::Photo(0, 0)));
        assert_eq!(step_photo(&counts, 0, 0, false), None);
    }

    #[test]
    fn thumb_url_name_percent_encodes_reserved_characters() {
        assert_eq!(
            thumb_url_name(Path::new("/d/IMG 01.jpg")),
            "IMG%2001.jpg.jpg"
        );
        assert_eq!(thumb_url_name(Path::new("/d/a#b.png")), "a%23b.png.jpg");
    }

    #[test]
    fn group_meta_lists_count_and_flags() {
        let mut is_bw = HashMap::new();
        is_bw.insert(PathBuf::from("/d/a.jpg"), true);
        let group = Group {
            photos: vec![photo("a.jpg", 0.0), photo("b.jpg", 0.1)],
        };
        assert_eq!(group_meta(&group, &is_bw), "2 photos · b&w · bordered");
        let plain = Group {
            photos: vec![photo("c.jpg", 0.0)],
        };
        assert_eq!(group_meta(&plain, &HashMap::new()), "1 photo");
    }

    #[test]
    fn plural_picks_the_right_form() {
        assert_eq!(plural(1, "photo", "photos"), "1 photo");
        assert_eq!(plural(2, "photo", "photos"), "2 photos");
        assert_eq!(plural(0, "photo", "photos"), "0 photos");
    }
}
