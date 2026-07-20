---
target: M3 arrangement editing (sequin-app)
total_score: 22
p0_count: 0
p1_count: 2
timestamp: 2026-07-20T02-18-24Z
slug: crates-sequin-app-src-main-rs
---
Method: dual-agent (A: opus design review · B: haiku detector)

## Design Health Score — M3 Arrangement Editing

| # | Heuristic | Score | Key Issue |
|---|-----------|-------|-----------|
| 1 | Visibility of System Status | 2 | Persistence communicated only on failure; undo silent |
| 2 | Match System / Real World | 3 | Light-table metaphor strong; bare M/S keys slightly arbitrary |
| 3 | User Control and Freedom | 2 | Redo undiscoverable; undo wipes selection/place |
| 4 | Consistency and Standards | 2 | No ARIA model on the editable surface; no shift-range-select |
| 5 | Error Prevention | 2 | Accidental-merge trap: whole group body is the merge target |
| 6 | Recognition Rather Than Recall | 2 | Hints line teaches ~half the model |
| 7 | Flexibility and Efficiency | 2 | One-step keyboard moves; no range select or move-to-top |
| 8 | Aesthetic and Minimalist Design | 4 | Photograph-per-pixel maximal; gold rationed |
| 9 | Error Recovery | 2 | Undo deep but silent; post-undo cursor jumps to top |
| 10 | Help and Documentation | 1 | One hints line; nothing else for a twice-a-year tool |
| **Total** | | **22/40** | **Acceptable — beautiful surface, real interaction/a11y gaps** |

## Anti-Patterns Verdict
**Not AI slop** (LLM + detector agree). No side-stripes, gradient text, glass, hero metrics, card grids, eyebrows; numbers are real timeline positions; surfaces pure chroma-0. Detector: 3 advisory findings only — font sizes off the DESIGN.md ramp (0.875rem ×2, 0.6875rem ×1 in style.css). Named-rule drift: DESIGN.md's Drag-Lift shadow and row-entrance stagger are specified but not shipped.

## Priority Issues
- **[P1] Keyboard users cannot merge (or multi-select at all).** Arrows replace selection; merge needs ≥2 groups; only cmd+click adds. Violates PRODUCT principle 5 and WCAG target. Fix: Shift+Arrow selection extension or Cmd+M "merge with group below", plus shift-click range select.
- **[P1] No ARIA on the editable surface.** Single tabindex=0 div; no role/label/live region; selection is CSS-only; all edits silent to AT; focus invisible when selection empty (WCAG 2.4.7). Fix: grid/listbox semantics + aria-activedescendant + polite live region announcing edits + visible focus independent of selection.
- **[P2] Accidental-merge trap.** Reorder target is the 36px gap; merge target is the entire group body — the safer action got the smaller hit-area. Fix: merge only on the group head (or with modifier); keep dashed warning.
- **[P2] No autosave/undo acknowledgment.** Saved state shown only on failure; undo/redo silent. Fix: quiet mono "Saved" in header; brief undo/redo announcement (also surfaces redo's existence).
- **[P3] Hints line incomplete; keyboard-move path undiscoverable.** Add cmd+arrow move, redo, Esc to hints or a "?" shortcuts popover.
- **[P3] Design-system drift.** Implement Drag-Lift custom drag preview + entrance stagger, or amend DESIGN.md to match shipped reality.

## Persona Red Flags
- **Alex (power user):** no range-select; one-step keyboard moves make 34-group reorders punishing; will trip the merge trap and never find redo.
- **Sam (keyboard + screen reader):** cannot merge at all; every edit silent; focus indicator disappears after Esc/undo.
- **Rex (owner, twice a year):** relearns from a hints line that teaches half the model; no ambient "Saved" reassurance for an afternoon of arranging.

## Minor Observations
14px append zone is narrow; gap-boundary insertion-line flicker risk (only GapZone clears hover on dragleave); illegal drops are silent no-ops; group index numbers faint (0.75rem) for primary wayfinding; no jump-to-group for long lists.

## Questions to Consider
1. Why does the app speak about persistence only when it fails?
2. Did the safer action (reorder) get the smaller hit-area by accident?
3. Is "a keyboard path exists" the same as "keys and mouse, equally" at 34 groups?

**Limitation:** interactive M3 states (insertion lines, merge outline, selection rings) reviewed from code only — screen was locked during capture; static M2-era frames verified visually.
