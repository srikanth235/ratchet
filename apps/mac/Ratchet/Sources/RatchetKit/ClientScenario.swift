import Foundation

/// How a test, a snapshot or the self-test drives the sample client without knowing it is one: make time pass, and make the
/// setup questions answer a particular way. `AppStore.scenario` holds the one that goes with its client (nil for the daemon),
/// so nothing downcasts `store.client` to reach the mock's internals.
public protocol ClientScenario: Sendable {
    /// Moves every running optimization one simulated minute forward. Returns what changed (also sent to the event stream).
    @discardableResult
    func advance() async -> [RatchetEvent]
    /// The keychain can (not) hold tokens and keys.
    func simulate(keychainAvailable available: Bool) async
    /// "What's missing" answers this list instead of working it out; nil goes back to working it out.
    func simulate(preflight items: [PreflightItem]?) async
    /// A world with nothing in it, for the empty states of a snapshot; `false` brings the sample data back (only if it was emptied).
    func simulate(empty: Bool) async
}
