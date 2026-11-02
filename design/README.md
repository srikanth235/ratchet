# Design reference — Midnight Brass

Source files of the six app screens from the "Ratchet App Design" canvas
(`.dc.html` Design Components). They are the visual reference the Mac app is checked against: `design/v02.html` is rendered
to `design/v02/ref-*.png` by `design/render-reference.mjs`, and `native.yml` compares the app's snapshots with them.

| File | Screen |
|---|---|
| `Main.dc.html` | 1 · Optimizations |
| `NewOptimization.dc.html` | 2 · New optimization |
| `Environment.dc.html` | 3 · AWS sandbox |
| `Live.dc.html` | 4 · Live agents |
| `Benchmarks.dc.html` | 5 · Benchmark history |
| `Result.dc.html` | 6 · Result & teardown |

## v0.3.1 mockups

`design/v03.html` is the source of the 22 screens of the v0.3.1 design (dark; Home, Overview and Run also in light). It is generated:

| Step | Command |
|---|---|
| Edit components / copy | `design/tools/build-v03.py`, styles and tokens in `design/tools/v03.css` |
| Build the page | `python3 design/tools/build-v03.py` |
| Build the clickable prototype | `python3 design/tools/build-prototype.py` → `design/v03-prototype.html` |
| Render PNGs | `node design/tools/render.js` → `design/v03/<n>-<name>.png` |
| Click test the prototype | `node design/tools/test-prototype.js` |
| Accessibility audit (contrast, size, targets, labels) | `node design/tools/audit.js` |

Tokens in `v03.css` are two layers named like `DesignKit/Tokens.swift`: Palette (`--night900`, `--brass500` …) and Ink (`--canvas`, `--sidebar`,
`--hairline`, `--controlBorder`, `--brassText`, `--onBrass`, `--off` …). Light mode redefines Ink only. Drift from Swift that F3 must adopt:

- `controlBorder` is new: slate 60% (dark) and umber 58% (light), 3:1 on canvas, sidebar, raised and glass. `hairline` and `hairlineStrong` stay for dividers.
- `glass` (popovers, tooltips, bubbles), `raisedSoft` (cards), `brassWash`, `brassLine`, `focus`, `scrim`, `okWash`/`badWash`/`waitWash`, the traffic-light colours and `mark*` are new names for values the mockup repeated.
- Light `mint700` is `#187250` (was `#1F8A62`, 4.1:1 as text) and light `ink500` is `#5C6676` (was `#6B7587`, 4.4:1 on paper). Both failed AA as text.
- Sizes: text 11, 12.5 and 14; button 32, small 28, field 36, sidebar row 32; radii 8, 12, 16; hover alpha .08.

## Kit and icon

`KIT.md` is the spec of the Swift design kit (tokens, type, icons, every part and its states); `DESIGN-SYSTEM.md` says which token to use when and where native macOS wins over the brand. The app icon has one source, `apps/mac/Ratchet/Icon/AppIcon.icon`; `python3 design/tools/build-icon.py` rewrites its layers and the flat `icon/ratchet-icon.svg`.
