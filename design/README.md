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

## v0.3.2 mockups

`design/v03.html` is the source of the 25 screens (dark; Home, Overview and Run also in light). It is generated. Since F7 it follows the shipped Mac app for the shell and
pages, and stays the reference for the draft card, the run timeline and the agents order editor:

- **Follows the app:** sidebar pages Home, Overview, Optimizations, Agents, Benchmarks, Connectors (no Customize) and Recent runs; native toolbar (title and subtitle, sandbox pill,
  one New Optimization, then page actions and page-scoped search); the Overview period picker and the Optimizations filter live on the page; four Settings tabs (General,
  Notifications, This Mac, Advanced); Agents, Connectors and Optimization settings as shipped.
- **Reference for the app:** draft card rows (Goal, Benchmark, Agents, Repository, Cloud, Schedule, Budget) with quiet Change actions, "Written by Claude Code · model", a footer that
  never wraps and a three-step drafting state; one run timeline with verdict and blocked cards and a steering composer docked under the scrolling log; the same agents order
  editor on Home and in the draft card; one sidebar selection style for pages and runs; "1 needs you".
- Screens: s1 Home, s9 Overview, s23 Optimizations, s2 Run, s3 Optimization settings, s4 Benchmarks, s5 Connectors, s7 Agents, s8 New Optimization, s24 draft with the order editor, s25 Menu bar, plus states.

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
