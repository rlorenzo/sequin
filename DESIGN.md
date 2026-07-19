---
name: Sequin
description: Put your photoshoot back in order — grouping, arranging, and timestamping studio deliveries before Apple Photos import.
colors:
  honey-gold: "oklch(0.817 0.161 75.1)"
  gold-hover: "oklch(0.775 0.155 75.1)"
  gold-hover-dark: "oklch(0.85 0.155 75.1)"
  gold-ink: "oklch(0.235 0.05 75.1)"
  accent-line: "oklch(0.56 0.125 75.1)"
  accent-line-dark: "oklch(0.817 0.161 75.1)"
  bg: "oklch(1 0 0)"
  bg-dark: "oklch(0.115 0 0)"
  surface: "oklch(0.955 0 0)"
  surface-dark: "oklch(0.165 0 0)"
  ink: "oklch(0.19 0 0)"
  ink-dark: "oklch(0.92 0 0)"
  muted: "oklch(0.47 0 0)"
  muted-dark: "oklch(0.67 0 0)"
  hairline: "oklch(0.89 0 0)"
  hairline-dark: "oklch(0.235 0 0)"
  danger: "oklch(0.5 0.19 27)"
  danger-dark: "oklch(0.72 0.16 25)"
typography:
  wordmark:
    fontFamily: "-apple-system, system-ui, sans-serif"
    fontSize: "0.9375rem"
    fontWeight: 650
    letterSpacing: "0.015em"
  title:
    fontFamily: "-apple-system, system-ui, sans-serif"
    fontSize: "1.375rem"
    fontWeight: 650
    letterSpacing: "-0.01em"
  body:
    fontFamily: "-apple-system, system-ui, sans-serif"
    fontSize: "1rem"
    fontWeight: 400
    lineHeight: 1.5
  label:
    fontFamily: "-apple-system, system-ui, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 500
  data:
    fontFamily: "ui-monospace, 'SF Mono', Menlo, monospace"
    fontSize: "0.75rem"
    fontWeight: 400
rounded:
  bar: "2px"
  thumb: "5px"
  control: "7px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
  2xl: "32px"
  3xl: "40px"
  4xl: "48px"
components:
  button-primary:
    backgroundColor: "{colors.honey-gold}"
    textColor: "{colors.gold-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "8px 18px"
  button-primary-hover:
    backgroundColor: "{colors.gold-hover}"
    textColor: "{colors.gold-ink}"
  button-quiet:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "6px 14px"
  button-quiet-hover:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
---

# Design System: Sequin

## 1. Overview

**Creative North Star: "The Light Table"**

Sequin is a photographer's light table: a quiet, glowing surface where the shoot is laid out, grouped, and slid into order. The photographs are the interface — chrome recedes to near-nothing, thumbnails carry all the color and meaning, and the app's own voice appears only where the photos can't speak: counts, badges, and one honey-gold glint on the single action that matters. The app follows macOS appearance: light mode is the white light table, dark mode is the darkroom — surfaces drop to pure near-black so thumbnails glow.

This system explicitly rejects the pro-app panel forest (Lightroom's docked intimidation), the generic web dashboard (SaaS cards, shadows, badges — "website in a window"), and sterile minimalism (mystery-meat icons, hidden functions). Familiar beats clever: it wears Apple Photos' grid conventions with the confident, image-first restraint of Darkroom and Halide.

**Key Characteristics:**
- Photos-first: every screen judged by how much of it is photograph
- Dual-theme, pure chroma-0 neutral surfaces; all warmth lives in the honey-gold accent
- One quiet system sans for UI; monospace strictly for data (counts, timestamps, filenames, badges)
- Responsive motion (150–280ms, ease-out expo), flat at rest
- WCAG 2.1 AA: contrast verified per theme, reduced-motion alternatives everywhere, keyboard-reachable controls

## 2. Colors

Restrained strategy: pure neutral surfaces, photographs as the palette, honey gold on a handful of pixels.

### Primary
- **Honey Gold** (oklch(0.817 0.161 75.1)): the color of a gold sequin catching light. Fills the primary action button (with near-black warm ink text) in both themes; draws the progress bar and focus rings in dark mode.
- **Deep Gold** (oklch(0.56 0.125 75.1), `accent-line`): the light-mode stand-in for gold on white — progress fill and focus rings need ≥3:1 against the surface, which bright gold can't give on white. Never used for body-size text.

### Neutral
- **Light table** (bg oklch(1 0 0), surface oklch(0.955 0 0), hairline oklch(0.89 0 0)): pure white ground; surface tints placeholder blocks and hover fills; hairline draws the single header rule and quiet button borders.
- **Darkroom** (bg oklch(0.115 0 0), surface oklch(0.165 0 0), hairline oklch(0.235 0 0)): near-black at chroma 0 — depth comes from lightness steps, never shadows.
- **Ink / Muted** (light: 0.19 / 0.47 · dark: 0.92 / 0.67): body and secondary text; both pairs hold ≥4.5:1 on their grounds, ink ≥7:1.

### Semantic
- **Danger** (light oklch(0.5 0.19 27), dark oklch(0.72 0.16 25)): error messages only, always in mono, always with a recovery action beside it.

### Named Rules
**The Ten-Percent Rule.** Honey gold touches at most 10% of any screen. If gold appears on more than the primary action, the progress bar, focus rings, and (future) selection, it has leaked.
**The Pure Surface Rule.** Surfaces are chroma-0 in both themes. Tinted-cream or tinted-charcoal backgrounds are prohibited; the photographs supply all environmental color.

## 3. Typography

**UI Font:** -apple-system / system-ui (SF Pro on macOS)
**Data Font:** ui-monospace / SF Mono / Menlo

**Character:** The sans disappears into the task; the mono is the light character — it marks everything the app reads or writes (photo counts, group badges, filenames, error output) the way frame numbers mark a contact sheet.

### Hierarchy
- **Title** (650, 1.375rem, -0.01em, `text-wrap: balance`): stage headings ("Put your shoot back in order.")
- **Wordmark** (650, 0.9375rem, +0.015em): the header "Sequin" only
- **Body** (400, 1rem, 1.5): stage prose, capped at 46ch, `text-wrap: pretty`
- **Label** (500, 0.8125rem): buttons and controls
- **Data** (400, 0.75rem, `tabular-nums`): group badges, counts, filenames, failure lists

Dark mode adds +0.008em letter-spacing to body text to compensate for light-on-dark perceived weight loss.

### Named Rules
**The Mono-Means-Data Rule.** Monospace is reserved for values the app reads or writes. UI labels never wear it; data never wears the sans.

## 4. Elevation

Flat. Surfaces sit on one plane in both themes; light mode separates with hairlines and surface tints, dark mode with lightness steps (bg 0.115 → surface 0.165). Nothing casts a shadow at rest, and no shadow vocabulary exists yet by design.

### Named Rules
**The Drag-Lift Rule.** The only pronounced shadow this app will ever have belongs to the thing being dragged (arriving with M3). A lifted photo or group casts a soft shadow while airborne and loses it on drop. If anything casts a shadow at rest, it is wrong.

## 5. Components

### Buttons
- **Shape:** gently rounded (7px), never pill, never square
- **Primary:** honey-gold fill, warm near-black ink (oklch(0.235 0.05 75.1)), 600 weight, 8px 18px padding. One per screen at most — it is the gold budget.
- **Quiet:** transparent with 1px hairline border, ink text, 6px 14px padding; hover fills with surface, active with hairline. The header "Open photo folder…" once a session exists.
- **States:** hover (150ms background ease), active (primary nudges 0.5px down), disabled (45% opacity, no hover), focus-visible (2px accent-line ring, 2px offset).

### Progress Bar
- 3px track in surface, 2px radius; determinate fill in accent-line driven by `transform: scaleX()` (never width); indeterminate variant sweeps a 40% segment at 1100ms. Reduced motion: static 45%-opacity full fill. Always paired with a mono count (`34 / 62`) and a quiet label.

### The Light Table (group rows)
- Each cluster is a `section.group`: a baseline-aligned head (bold mono index — a real timeline position — plus muted mono badges `3 photos · b&w · bordered`) over a `flex-wrap` row of thumbnails, 8px inside, 40px between groups. No cards, no rules, no containers — proximity does the grouping.
- Rows enter with a 280ms rise (6px translate + fade), staggered 24ms per row, capped at row 14. Reduced motion: instant.

### Thumbnails
- 176px tall, natural aspect (locked via inline `aspect-ratio` before load — zero layout shift), 5px radius, surface-colored placeholder, `loading="lazy"`, filename in `alt` and `title`. Served from the local cache via the `/thumbs/` asset handler.

### Failure Disclosure
- A native `<details>` above the grid: muted summary ("2 files couldn't be read"), mono filenames with reasons inside. Never a modal, never a toast.

## 6. Do's and Don'ts

### Do:
- **Do** let thumbnails dominate: maximize photograph-per-pixel on every screen.
- **Do** use Apple Photos' familiar grid conventions — earned familiarity over invention.
- **Do** keep transitions 150–280ms with `cubic-bezier(0.16, 1, 0.3, 1)` and give every animation a `prefers-reduced-motion` alternative.
- **Do** give every interactive element default, hover, focus-visible, active, and disabled states, keyboard-reachable.
- **Do** pair every error with a recovery action ("Try another folder…").

### Don't:
- **Don't** build the "pro-app panel forest" — no docked panels, no control walls (PRODUCT.md anti-reference).
- **Don't** build the "generic web dashboard" — no SaaS cards, badges, or decorative shadows; nothing that reads "website in a window" (PRODUCT.md anti-reference).
- **Don't** slide into "sterile minimalism" — no mystery-meat icons, no unlabeled functions, no form over usability (PRODUCT.md anti-reference).
- **Don't** tint the surfaces; The Pure Surface Rule stands. Warmth is honey gold's job.
- **Don't** put bright honey gold on white for lines or text — use Deep Gold (`accent-line`) there.
- **Don't** decorate with motion. If an animation doesn't convey state, feedback, or the single completion reward (M4), it doesn't exist.
