import Foundation

/// A `ratchet://` link: how the `ratchet open` command, a script or another app asks the window to show something.
///
///   ratchet://open/<page or optimization id>   a page (`overview`, `optimizations`, ...), or an optimization by its id
///   ratchet://optimization/<id>                one optimization
///   ratchet://connectors | new | overview      the same places a notification click opens (`NotificationDestination`)
///   ratchet://<page>                           a page by its name, e.g. ratchet://benchmarks
///
/// Parsing reuses `NotificationDestination.init(token:)`, so a link and a notification click can never disagree about
/// what "connectors" or "new" means. The pages themselves belong to the app (the sidebar), so a link that names a page
/// stays a name until `resolve(pages:)` is given the pages the app has.
public enum AppLink: Equatable, Sendable {
    /// A place a notification click can also open.
    case destination(NotificationDestination)
    /// A page name or an optimization id, still to be told apart (see `resolve(pages:)`).
    case open(String)

    public static let scheme = "ratchet"

    /// What a link means once the pages are known.
    public enum Target: Equatable, Sendable {
        case page(String)
        case destination(NotificationDestination)
    }

    public init?(url: URL) {
        guard url.scheme?.lowercased() == Self.scheme else { return nil }
        var parts: [String] = []
        if let host = url.host(percentEncoded: false), !host.isEmpty { parts.append(host) }
        parts += url.pathComponents.filter { $0 != "/" && !$0.isEmpty }
        guard let verb = parts.first else { return nil }
        let rest = Array(parts.dropFirst())
        switch verb.lowercased() {
        case "open":
            guard let name = rest.first else { return nil }
            self = .open(name)
        case "optimization":
            guard let id = rest.first else { return nil }
            self = .destination(.optimization(id))
        default:
            if let destination = NotificationDestination(token: verb.lowercased()) {
                self = .destination(destination)
            } else {
                // `ratchet://benchmarks`: a page by its name.
                self = .open(verb)
            }
        }
    }

    /// Tells a page name from an optimization id. A page name wins, whatever its case; the places a notification can open
    /// (`connectors`, `new`, `overview`) come next; anything else is an optimization id.
    public func resolve(pages: [String]) -> Target {
        switch self {
        case .destination(let destination):
            return .destination(destination)
        case .open(let name):
            let lower = name.lowercased()
            if pages.contains(lower) { return .page(lower) }
            if let destination = NotificationDestination(token: lower) { return .destination(destination) }
            return .destination(.optimization(name))
        }
    }

    private static func encode(_ text: String) -> String? {
        text.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-._~")))
    }

    /// The link that opens `target`, a page name or an optimization id (what `ratchet open` builds).
    public static func url(opening target: String) -> URL? {
        let trimmed = target.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let encoded = encode(trimmed) else { return nil }
        return URL(string: "\(scheme)://open/\(encoded)")
    }

    /// The link for a place a notification can open.
    public static func url(for destination: NotificationDestination) -> URL? {
        switch destination {
        case .optimization(let id): encode(id).flatMap { URL(string: "\(scheme)://optimization/\($0)") }
        case .connectors, .newOptimization, .overview: URL(string: "\(scheme)://\(destination.token)")
        }
    }
}
