# Ratchet design system

Which token to use when. Parts and their states: `design/KIT.md`. Look: Midnight Brass, calm, v0.3.1 (`design/tools/v03.css`).
Tokens live in `apps/mac/Ratchet/Sources/Ratchet/DesignKit/Tokens.swift`; `tools/grid-lint.sh` enforces the rules marked (lint).

## Accent decision

Brass is the brand and the only accent. `.tint(Ink.brass)` sits on the main window, Settings and the menu bar window, so system controls (toggles,
pickers, selection, focus) wear brass, not the Mac's accent colour. People do not choose a colour: the product has one identity, and brass was
checked for contrast in both modes. Revisit only if a user-facing accent picker is ever specced.

## Native macOS or Midnight Brass

| Area | Rule that wins |
| --- | --- |
| Title bar, toolbar, menus, alerts, sheets, popovers (chrome) | native macOS. No custom backgrounds behind them |
| Settings window (`Form`, tabs) | native layout and controls; text through `.kitType`, colour through `Ink`; 520 wide |
| Sidebar, toolbar, window title | native macOS (F4): the system sidebar (`List` with `.listStyle(.sidebar)`), a `.toolbar`, `.navigationTitle`. No background or edge of our own; brass only in the content. One selection style, the system's, for pages and runs |
| Tables and lists of optimizations | native `Table` (selection, sorting, context menu, drag); cell text through `.kitType`, colour through `Ink` |
| Pages: Overview, Optimizations, Benchmarks, Connectors, detail | brand: kit page, parts, serif titles |
| Composer, steering field | brand parts on Liquid Glass: `.glassEffect()` inside one `KitGlassContainer`, control edge and focus ring stay the kit's. Never glass on glass |
| Draft card, chips | brand parts |
| Menu bar window | native window, brand content |
| Dock icon, menu bar mark | the app icon / template mark |

## Chrome that follows the window

- Sheets, popovers, bars, the menu bar window: no `.background` (the system paints glass). Custom fills are for content only.
- Custom chrome (page titles, status items, segments, the daemon banner) calls `.dimsWhenInactive()` (`appearsActive`, `Dim.inactiveOpacity`).
- Radii nest through `ConcentricRectangle` inside a `.containerShape`; one `Layout.contentWidth` per page; the page margin is one step, `Space.page` (40), at the sides, top and bottom.
- Inputs that stay at the bottom edge are docked under the scroll view (`KitPage(bottomBar:)`), not floating over it: the content stops above them. Side columns use `ViewThatFits`.

## Colour (Ink)

- Never a hue, a hex, `Color(red:)`, `.secondary`, `.red` or `Palette.` outside `Tokens.swift` (lint).
- Text: `text` (body), `textMuted` (labels), `textFaint` (meta, 5.6:1 or more). Accent text is `brassText`; text on a brass fill is `onBrass`.
- Fills: `canvas` page, `raised`/`raisedSoft` cards, `glass` popovers. Selection: `brassWash` + `brassBorder`.
- Lines: `hairline` between rows, `hairlineStrong` around cards, `controlBorder` around anything you can click or type in (3:1, WCAG 1.4.11).
- Status: `ok` `wait` `bad` `off` for text and shapes; `*Wash` behind banners. Status is never colour alone: a shape or the word comes with it.
- Every colour has light, dark and Increase Contrast values; add all three when adding a token.

## Type

Only `.kitType(role)`; roles by purpose (title, body, meta, caption, micro, mono). Text sizes are 11, 12.5 and 14; 10 pt is the floor (chart axes only). Serif for titles
and big numbers only. Numbers use the mono role or `.monospacedDigit()`. Mono is SF Mono.

## Sizes (Dim, Layout, Space)

- Spacing from `Space` (4 pt grid); control sizes and radii from `Dim`; widths shared by screens from `Layout` (sheet 520, label column, number columns, minimum window) (lint).
- A new size is a named token with a reason; a number off the grid needs `// grid: ok <why>`.
- Radii nest: inner radius = outer radius minus padding. One `disabledOpacity`.

## Controls

- Real controls only: `Button` + `ButtonStyle`, `Toggle` + `KitToggleStyle`, `TextField` + `.kitField()`. Never a shape with a tap gesture.
- One primary button per page or card. Icon-only buttons are `KitIconButton` with a label.
- Every part takes `forcedPartState` and has all six states in the gallery before it ships in a screen.
- Focus: the brass 2 pt ring (`kitFocusRing`) on every focusable part; keyboard reaches everything; Left/Right in segments, Up/Down in lists.

## Motion

- Only through `Motion` (`hover`, `press`, `step`, `appear`, `loop`). All are nil under Reduce Motion. No raw `.animation(.easeOut(duration:))`.
- Hover never moves a selection. A spinner stops under Reduce Motion but stays visible.

## Gallery and checks

`APP_KIT_GALLERY=1 Ratchet` shows every part in every state; snapshot mode shoots the `gallery-*` scenes in light and dark. Add the part to
`KitGallery.swift` and `KIT.md` in the same change. Checks: `tools/grid-lint.sh`, the layout probes (`tools/check-layout.swift`), the gallery snapshots.
