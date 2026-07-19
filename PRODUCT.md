# Product

## Register

product

## Platform

web

## Users

Primary: Rex — the owner-operator. A macOS user who, roughly twice a year, downloads a studio photo delivery (dozens of JPEGs with UUID filenames and identical timestamps) and wants them in Apple Photos in shoot order. Context: a just-downloaded folder, a fresh memory of the shoot, and maybe twenty minutes of patience. The job: verify the automatic groups, drag everything into shoot order, write the timestamps, import.

Secondary: strangers with the same problem — open-source users today, possible Mac App Store buyers later (the Maccy model). They arrive with zero training; the app must be instantly understandable on first launch.

## Product Purpose

Sequin is a macOS desktop app (Dioxus webview) that fixes studio photo deliveries before Apple Photos import: it groups the styled variants of each shot via perceptual hashing, lets the user arrange groups in shoot order, and writes sequential EXIF capture times. Apple Photos sorts strictly by capture time and cannot fix ordering after import — Sequin exists to fix it before. Success: a delivery goes from chaotic folder to correctly-ordered Photos timeline in one short, pleasant session.

## Positioning

The only tool that combines similarity grouping, drag-to-reorder, and sequential EXIF time writing in one step — what currently takes a $60 two-app round trip with no visual grouping.

## Brand Personality

Precise, effortless, quietly sparkly. A confident small utility in the spirit of Darkroom and Halide — image-first, opinionated, zero bloat — wearing Apple Photos' familiar grid conventions so it feels like a natural pre-Photos companion. The personality lives in micro-moments (a shimmer when the timeline is written, a warm accent), never in decoration that competes with the photos. Voice: plain, direct, no jargon.

## Anti-references

- Pro-app panel forest (Lightroom, Capture One): docked panels, hundreds of controls, intimidating on open.
- Generic web dashboard: SaaS cards, shadows, badges — "website in a window."
- Sterile minimalism: mystery-meat icons, hidden functions, form over usability.

## Design Principles

1. **Photos are the interface.** Thumbnails carry the meaning; chrome recedes. Every screen is judged by how much of it is photograph.
2. **The whole job on one screen.** Group → order → write times is one linear flow, visible at once. No modes, no panels, no navigation to learn.
3. **Trust before writing.** EXIF writes are the only irreversible act — always preview, default to copies, verify after. The user should never wonder what just happened to their originals.
4. **Delight at the finish line.** Sparkle belongs to completed work — micro-moments of reward, never ornament in the way of the task.
5. **Hands on keys or mouse, equally.** Drag-and-drop is the hero interaction, but every rearrangement has a keyboard path.

## Accessibility & Inclusion

WCAG 2.1 AA is the formal target, audited as part of the workflow: text contrast ≥ 4.5:1 (3:1 for large text), `prefers-reduced-motion` alternatives for all animation including the completion sparkle, and full keyboard operability of the reorder UX so drag-and-drop is never the only path.
