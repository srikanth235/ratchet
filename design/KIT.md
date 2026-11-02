# Ratchet design kit

The spec of `apps/mac/Ratchet/Sources/Ratchet/DesignKit/`, at v0.3.1. Mockup source: `design/tools/v03.css` (two token layers named like
`Tokens.swift`). Which token to use when: `design/DESIGN-SYSTEM.md`. Every part below is drawn in every state by the gallery
(`APP_KIT_GALLERY=1`, or the `gallery-*` snapshot scenes in light and dark).

## Feel

Calm, exact, warm. A dark instrument panel with one brass accent, and the same palette on warm paper in light mode. Serif for titles
and big numbers, sans for text, numbers in the system mono. Lines are quiet (dividers) except around controls, which are always findable.

## Tokens

Three layers. `Palette` (raw, only `Tokens.swift` reads it), `Ink` (meaning, light + dark + Increase Contrast), `Dim`/`Layout`/`Motion`.

### Ink (dark / light)

| Token | Dark | Light | Used for |
| --- | --- | --- | --- |
| `canvas` | night900 `#0B1019` | paper `#FBFAF6` | page background |
| `sidebar` | night850 | paperDeep `#F3F1EA` | sidebar surface |
| `raised`, `raisedSoft` | night800, night850 | white | cards, composer, fields |
| `glass` | night700 | white | popovers, tooltips, bubbles |
| `hairline`, `hairlineStrong` | slate400 11%, 20% | umber 14%, 24% | dividers; card edges. Never a control outline |
| `controlBorder` | slate400 60% | umber 58% | every control outline: 3.1 to 3.5:1 on canvas, sidebar, raised, glass (1.4.11) |
| `hover`, `track` | slate400 8%, 14% | umber 6%, 12% | hover fill (alpha .08 dark); bar and pressed fill |
| `text`, `textMuted`, `textFaint` | snow, slate300, slate500 | ink900, ink600, ink500 `#5C6676` | 16.5 / 8.8 / 5.6:1 dark; 17 / 7.4 / 5.6:1 light |
| `brass`, `brassFill` | brass500 `#D9B26F` | brass600 / brass500 | fills, chart line, selection marks |
| `brassText`, `onBrass` | brass300, brassInk | brass700 `#8A5E0B`, brassInk | brass text; text on a brass fill (9.4:1) |
| `brassWash`, `brassBorder` | brass500 6%, 50% | brass600 8%, brass700 50% | selected card or row; hover and open edge |
| `focus` | brass300 | brass700 | focus ring (2 pt, 2 pt outside) |
| `ok`, `wait`, `bad`, `off` | mint400, amber400, rose400, gray600 | mint700 `#187250`, amber700 `#956108`, rose700, gray400 | status text and shapes; all four are 4.5:1 or more as text on paper |
| `okWash`, `waitWash`, `badWash` | 10-12% of the status color | 8-10% | banner and error backgrounds |
| `scrim` | `#030509` 62% | umber 32% | behind dialogs |
| `markGlass`, `markHi`, `markMid`, `markLo` | same in both | same | the brand mark only |

Increase Contrast: `hairline`/`hairlineStrong` 40%/60%, `controlBorder` 90%+, `brassBorder` 100%, `textMuted`/`textFaint` one step stronger,
`brass`/`brassText` in light one step darker. Deviation from `v03.css`: light `amber700` is `#956108` (the mockup's `#A86F0B` is 4.1:1 as text).

### Dim, Layout, Motion

Radii: `radiusXS` 4 (checkbox, tag), `radiusButton`/`Row`/`Field` 8, `radiusCard` 12, `radiusLarge` 16 (composer, window). Heights: button 32, small 28
(the smallest hit target), field 36, sidebar row 32, chip 28, segmented 32, send 32, toggle 40 x 24 (knob 16). One `disabledOpacity` .45. Focus ring 2 / offset 2.
Status dot 8, status shape 16, bar 4, mark 16, icon 16. Row padding 12, section gap 12, card gap 12. Everything is 0, 1, 2 or a multiple of 4 (`tools/grid-lint.sh`);
the exceptions say why (text line heights 18, stroke 1.5).

`Layout`: sheet 520, Settings 520, popover 320, settings sidebar 176, label column 160 (128 in a draft card), number columns 80 and 56, side
columns 360 and 320, content 1000, composer 720, window 1280 x 820 (minimum 1080 x 640).

`Motion`: `hover` .12 s, `press` .08, `step` .2, `appear` .24, `loop` (spinner). All are nil under Reduce Motion: states still change, nothing moves.

## Type

| Role | Face | Size | Used for |
| --- | --- | --- | --- |
| `pageTitle`, `brand`, `quote`, `heroUnit` | Instrument Serif | 32, 23, 26, 22 | titles |
| `hero` | Instrument Serif | 56 | the big numbers |
| `body`, `bodyStrong` | Instrument Sans | 13.5 | rows, fields |
| `meta`, `caption`, `captionStrong` | Instrument Sans | 12.5, 12, 12 | notes, labels |
| `section`, `button`, `buttonPrimary`, `buttonSmall` | Instrument Sans | 13, 13, 13, 12 | headings, buttons |
| `micro`, `microStrong` | Instrument Sans | 11 | badges, counters, group headings |
| `mono`, `monoStrong`, `microMono` | SF Mono (system) | 12.5, 12.5, 11 | numbers, commands. Decision: SF Mono; the mockup's JetBrains Mono is not bundled |
| `axis`, `axisMono` | Instrument Sans, SF Mono | 10 | chart axes. Nothing is smaller than 10 pt |

The mockup scale is 11 / 12.5 / 14. The app keeps 12 (`caption`) and 13.5 (`body`) until F4 merges them into 12.5 and 14 with the screens.

## Icons

Drawn `Shape`s on a 16 x 16 box, one 1.5 round stroke, never an SF Symbol outside `DesignKit/` (`grid-lint.sh` checks). Views name a `KitIcon`; `KitIconView`
sets size and weight. Set: overview, optimizations, benchmarks, connectors, check, back, plus, chevronDown, arrowUp, close, sliders, bell, terminal. An icon-only
button is a `KitIconButton`, which requires a label (VoiceOver and tooltip).

## Parts

States for every part: rest, hover, pressed, selected, focused, disabled. `forcedPartState` (environment) forces one for the gallery. Focus is the brass 2 pt ring
(`kitFocusRing`), drawn outside controls and inside full-width rows. Disabled is `disabledOpacity` and no hover.

| Part (file) | Anatomy and size | Notes |
| --- | --- | --- |
| Primary button (`Buttons`) | 32 high, 16 side padding, `radiusButton`, brass fill, `onBrass` text | hover .92, pressed .8. One per page or card |
| Quiet button | same, `controlBorder` edge, transparent | hover: `brassBorder` + `hover` fill; pressed `track` |
| Ghost, Danger | no edge; red text on a quiet edge | Danger is for Stop and Delete |
| Link | `brassText`, underline on hover | text only, 4 pt hit padding |
| Small variants | 28 high, 12 padding | |
| Icon button (`KitIconButton`) | 32 square (28 small), `controlBorder` | always labelled |
| Choice card (`Choice`) | radio or checkbox mark 16, title + meta, `radiusCard`, `hairlineStrong` edge | selected: `brassWash` + `brassBorder`; the whole card wears the focus ring |
| Step progress | segments 28 x 4, done in brass | |
| Sidebar row (`Rows`) | 32 high, icon 16, `body`, `radiusRow` | selected: `hover` fill (dims when the window is inactive), brass icon |
| Table row | full width, hairline under, `hover` fill | focus ring inside |
| Hairline list and row | 12 vertical padding | |
| Status label | dot (8) or shape (16) + word + caption | the word carries the meaning |
| Status shape (`Controls`) | circle with tick (done), turning arc (running), diamond (waiting), triangle (failed), dashed circle (off) | colour-independent, 1.4.1 |
| Field (`Page`) | 36 high, `radiusField`, `controlBorder`, focus: brass edge + ring | single line |
| Composer (`Controls`) | `radiusLarge` card, multi-line field (2 to 6 lines), chips row, 32 send button | Command-Return sends; sending shows a stop button |
| Popover chip | 28 high, title + chevron, `controlBorder` | open: brass edge; the caller attaches the popover |
| Segmented control | 32 high, bordered, segments split by `hairlineStrong` | selected: lifted + brass underline; Left/Right move |
| Range switch (`KitSwitch`) | text-only segments | chart ranges |
| Toggle (`KitToggleStyle`) | 40 x 24 track, knob 16 | on: brass track, dark knob; real `Toggle` |
| Settings sidebar | heading (`microStrong`) + sidebar rows, 176 wide | Up/Down move |
| Page, header, section, hero number, thin bar | see `Page.swift` | |
| Mark (`RatchetMark`) | six chambers; the newest in brass | colours from `Ink.mark*`; menu bar image is a template |

## Window and icon

Chrome (title bar, toolbar, menus, sheets, popovers) is system. The window opens at 1280 x 820, minimum 1080 x 640. `.tint(Ink.brass)` is set on the main window,
Settings and the menu bar window.

App icon: one source, `apps/mac/Ratchet/Icon/AppIcon.icon` (`Assets/*.svg` come from `design/tools/build-icon.py`, which also writes the flat `design/icon/ratchet-icon.svg`).
Four chambers (the two smallest of the mark are left out: half a pixel at 16 pt), glyph 50% of the canvas, glass layers at 80%. A dark fill specialization is
`fill-specializations` in `icon.json`; check all looks with `tools/icon_review.sh` on a Mac (the CI "Icon review" step renders them).
