import Foundation
import Observation

/// The one error line the window shows (`ErrorBanner`), shared by the stores. A failed action says what it was doing and stays
/// until the person closes it or starts the next action; a failed load clears itself when the next load works, so a background
/// refresh never wipes an action's error the person has not read.
@MainActor @Observable
public final class StoreErrors {
    public private(set) var message: String?
    /// True while `message` came from loading data.
    @ObservationIgnored private var cameFromLoading = false

    public init() {}

    /// Every action starts here, so the banner never shows the answer to something the person did before.
    public func beginAction() { clear() }

    public func clear() {
        message = nil
        cameFromLoading = false
    }

    /// An action failed: "Couldn't stop the optimization. <reason>".
    public func fail(_ error: Error, _ headline: String? = nil) {
        let reason = error.localizedDescription
        message = headline.map { "\($0). \(reason)" } ?? reason
        cameFromLoading = false
    }

    /// Something the views did (saving a file) failed: shown like any other error.
    public func report(_ text: String) {
        message = text
        cameFromLoading = false
    }

    /// Loading failed.
    public func failLoading(_ text: String) {
        message = text
        cameFromLoading = true
    }

    public func failLoading(_ error: Error) { failLoading(error.localizedDescription) }

    /// A load worked: its own earlier failure is gone, an action's is kept.
    public func loadSucceeded() {
        if cameFromLoading { clear() }
    }
}
