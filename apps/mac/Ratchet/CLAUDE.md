# Ratchet (Mac app)

The native macOS app of Ratchet, in SwiftUI. Plan: `docs/NATIVE_ARCHITECTURE.md` ("Mac app"). Look: Midnight Brass, calm
version (`design/v02.html`, `design/v02/*.png`). It lives in the Ratchet repo; there is no separate git repo.

## Data

No SwiftData. Data comes from `ratchetd` through the `RatchetClient` protocol (`Sources/RatchetKit/RatchetClient.swift`):
REST-shaped resources plus one event stream. Today the only implementation is `MockRatchetClient` (sample data, fake live
events). The client generated from `ratchetd`'s OpenAPI replaces it in `Store.makeStore()` (`App.swift`); nothing else
changes. `AppStore` (`@Observable`) renders the last snapshot (`last-snapshot.json`) at once, refreshes, then applies events.

## Layout

- `Sources/RatchetKit/` models, formatting, client protocol, mock, `AppStore`, `NewOptimizationFlow`. No SwiftUI. Tests in `Tests/RatchetKitTests/`.
- `Sources/Ratchet/` the app: `App.swift` (scenes, commands, `Store`), `ContentView.swift`, `Sidebar.swift`, `Router.swift`, `MenuBarView.swift`, `SettingsView.swift`, `Snapshot.swift`, `SelfTest.swift`.
- `Sources/Ratchet/Screens/` one file per screen: Overview, Optimizations, detail, New optimization steps, Benchmarks, Connectors.
- `Sources/Ratchet/DesignKit/` the design kit: `Tokens.swift` (Palette, Ink, Dim, Motion), `Type.swift`, `Icons.swift`, `Buttons.swift`, `Rows.swift`, `Choice.swift`, `Page.swift`.
- `Sources/Ratchet/UI/` `Grid.swift` (the 4 pt scale, layout probes), `WindowBehavior.swift`.

## Build and check

- Build and install: `./build.sh` (`--open` restarts the app). Prints the compiler errors with context on failure.
- Unit tests: `swift test --scratch-path ~/Library/Developer/Ratchet/test-build`.
- Self-test: `tools/selftest.sh` (exit 0 when every check passes). Add a check for every story with logic.
- Snapshots: `tools/snap.sh` writes `/tmp/Ratchet-shots/<scene>-light.png` and `-dark.png` for the four pages, the optimization detail (live, finished, no win), the four New optimization steps, and `settings-<tab>-*.png`. `APP_SNAPSHOT_PAGES=overview,new-budget tools/snap.sh` limits the scenes.
- Grid: `tools/grid-lint.sh`, then `swift tools/check-layout.swift /tmp/Ratchet-shots/*-layout.json`.
- Test mode: `APP_STORE=<file in /tmp>` means a fixed clock, no live ticks and no saved snapshot.
- CI: `.github/workflows/native.yml`, job `mac-app` (macOS 26).

## Rules

- Spacing comes from `Space` (`UI/Grid.swift`), on a 4 pt grid; the design's own odd sizes (3, 7, 15 pt) live in `DesignKit/Tokens.swift` as `Dim`.
- Colors only through `Ink` (light and dark values), fonts only through `.kitType(...)`, icons through `KitIcon`.
- Parts are real controls (`Button` with a `ButtonStyle`), never a shape with a tap gesture.
- Window chrome (title bar, toolbar, menus, sheets, popovers) stays system. Every action has a menu item and a shortcut: Command-N new optimization, Command-1 to 4 pages.
- Money, percents and durations go through `Money`, `Percent`, `Elapsed` in the Kit, so the tests cover them.
- Main-actor default isolation in the app target; the Kit target is nonisolated, its types are `Sendable` structs and one actor (`MockRatchetClient`).
