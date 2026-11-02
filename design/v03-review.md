# v0.3.1 mockup review

Two passes over `design/v03.html` (22 screens, dark and light): design critique and accessibility (WCAG 2.1 AA). The first pass found the 🔴 items
below; the second pass, after the fixes, found none. The `design:*` skills were not installed in this session, so both passes ran their
criteria by hand plus `design/tools/audit.js` (computed styles in a browser) and `design/tools/test-prototype.js` (552 clicks).

## Design critique

| | Finding | State |
|---|---|---|
| 🔴 | Home and Overview told two value stories | Fixed: "$12.5k delivered + $2.9k pending · 68×" on both |
| 🔴 | Creation card waited on a PR | Fixed: config goes to `ratchet/config` and runs at once |
| 🔴 | Sidebar mixed actions with navigation, 3 levels, runs and optimizations alike | Fixed: New and search in the toolbar, 2 levels, runs prefixed `#14`, selected run has a bar and outline |
| 🔴 | Settings was a 13-section modal that duplicated Customize | Fixed: 520 pt window, 4 tabs, no Customize pages |
| 🔴 | Draft card: six "change" links, no Undo, no Stop, no error | Fixed: "Change agents" etc., popover, Updated + Undo, Start Over, Stop, inline error with fixes |
| 🔴 | Steering gave no feedback; settings rows looked read-only | Fixed: timeline with read receipt and Undo; Edit buttons, Saved, Default/Custom |
| 🟡→✅ | Two right edges, several primary buttons, bottom-edge sandbox line, happy paths only | Fixed: one `--w` per screen; one brass action per screen; toolbar sandbox pill; empty, loading, daemon-down, failed-connector, error and stop screens added |
| 🟡 | Defaults and Notifications settings tabs have no PNG | Open: built as panes in the prototype only |
| 🟡 | Overview is 1,400 px tall; the window grows instead of scrolling | Open: the app scrolls it; F4 decides the fold |
| 🟡 | Inputs use a 20 pt bottom inset in the mockup; the app must use `.safeAreaInset` | Open: F4 |
| 🟢 | Fonts: Instrument Sans is not installed where the PNGs render, so they use the fallback sans | Open |

## Accessibility

| | Check | Result |
|---|---|---|
| ✅ | 1.4.3 text contrast | 1,823 text runs pass 4.5:1 (3:1 for large) in dark and light |
| ✅ | 1.4.11 control borders | 234 controls pass 3:1 with `--controlBorder`; `--hairline` was 1.38:1 |
| ✅ | 1.4.1 colour alone | Every status is a shape plus a word (check, ring, half, cross, dash, dashed) |
| ✅ | 1.4.4 text size | Minimum 11 px; sizes 11, 12.5, 14 |
| ✅ | 1.1.1 chart | `role=img` with title, description and caption; "Spent $184" and "pending" are drawn as text |
| ✅ | 2.4.7 focus | Rings drawn on the Home composer and a Sign In button (s1, s13); the prototype uses `:focus-visible` |
| ✅ | 4.1.2 labels | No unlabelled icon-only button; popovers, dialogs, tabs and switches carry roles |
| ✅ | 2.5.5 targets | Arrows 36 px, buttons ≥28 px, switches 40×24 |
| 🟡 | Light palette | `mint700` and `ink500` were 4.1 and 4.4:1 as text; the mockup uses `#187250` and `#5C6676`. F3 must change `Tokens.swift` |
| 🟡 | Increase Contrast, Reduce Motion | Reduce Motion is honoured in CSS; no Increase Contrast render |
| 🟡 | Keyboard checklist (Tab order, ⌘↩, Esc, ⌘,) | The prototype handles ⌘,, Esc, Enter and Space; the app must repeat it |
