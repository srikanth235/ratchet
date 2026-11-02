import SwiftUI

/// Home: the composer and the conversation. P5b fills this view; the shell (sidebar row, window title, ⌘1 shortcut, toolbar
/// and daemon banner) is already around it. The floating composer goes in a `KitGlassContainer` (see `KitComposer`) anchored
/// with `.safeAreaInset(edge: .bottom)`.
struct HomePage: View {
    var body: some View {
        PagePlaceholder(page: .home, note: "Describe what to optimize and Ratchet drafts it with you. This screen is coming next.")
    }
}
