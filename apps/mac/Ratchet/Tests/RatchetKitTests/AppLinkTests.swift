import Foundation
import Testing
@testable import RatchetKit

@Suite("ratchet:// links")
struct AppLinkTests {
    private let pages = ["overview", "optimizations", "benchmarks", "connectors"]

    private func parse(_ text: String) -> AppLink? {
        URL(string: text).flatMap(AppLink.init(url:))
    }

    @Test func openTakesAPageOrAnId() {
        #expect(parse("ratchet://open/benchmarks") == .open("benchmarks"))
        #expect(parse("ratchet://open/opt-Checkout") == .open("opt-Checkout"))
        #expect(parse("ratchet://open") == nil)
    }

    @Test func theNotificationPlacesUseTheNotificationParser() {
        #expect(parse("ratchet://connectors") == .destination(.connectors))
        #expect(parse("ratchet://new") == .destination(.newOptimization))
        #expect(parse("ratchet://overview") == .destination(.overview))
        #expect(parse("ratchet://optimization/opt-1") == .destination(.optimization("opt-1")))
        #expect(parse("ratchet://optimization") == nil)
    }

    @Test func aPageCanBeNamedByItself() {
        #expect(parse("ratchet://benchmarks") == .open("benchmarks"))
        #expect(parse("ratchet:///benchmarks") == .open("benchmarks"))
    }

    @Test func otherSchemesAndEmptyLinksAreIgnored() {
        #expect(parse("https://open/overview") == nil)
        #expect(parse("ratchet://") == nil)
        #expect(parse("RATCHET://CONNECTORS") == .destination(.connectors))
    }

    @Test func resolvingTellsPagesFromIds() {
        #expect(AppLink.open("Benchmarks").resolve(pages: pages) == .page("benchmarks"))
        #expect(AppLink.open("overview").resolve(pages: pages) == .page("overview"))
        #expect(AppLink.open("new").resolve(pages: pages) == .destination(.newOptimization))
        #expect(AppLink.open("opt-checkout").resolve(pages: pages) == .destination(.optimization("opt-checkout")))
        #expect(AppLink.destination(.connectors).resolve(pages: []) == .destination(.connectors))
    }

    @Test func linksRoundTripThroughTheirURL() throws {
        let destinations: [NotificationDestination] = [.optimization("opt a_b"), .connectors, .newOptimization, .overview]
        for destination in destinations {
            let url = try #require(AppLink.url(for: destination))
            let link = try #require(AppLink(url: url))
            #expect(link.resolve(pages: []) == .destination(destination), "\(url)")
        }
        let url = try #require(AppLink.url(opening: "opt-1"))
        #expect(url.absoluteString == "ratchet://open/opt-1")
        #expect(AppLink.url(opening: "  ") == nil)
    }
}
