// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Ratchet",
    platforms: [.macOS(.v26)],
    dependencies: [
        // The runtime the generated client in RatchetAPI is written against (Sources/RatchetAPI, tools/generate-api.sh).
        .package(url: "https://github.com/apple/swift-openapi-runtime", from: "1.13.0"),
        .package(url: "https://github.com/apple/swift-http-types", from: "1.7.0"),
    ],
    targets: [
        // Types and client generated from api/openapi.yaml by swift-openapi-generator. Committed, never edited by hand.
        .target(
            name: "RatchetAPI",
            dependencies: [
                .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime"),
                .product(name: "HTTPTypes", package: "swift-http-types"),
            ]
        ),
        // Rules and data logic with no SwiftUI, so tests run without the app.
        .target(
            name: "RatchetKit",
            dependencies: [
                "RatchetAPI",
                .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime"),
                .product(name: "HTTPTypes", package: "swift-http-types"),
            ]
        ),
        .executableTarget(
            name: "Ratchet",
            dependencies: ["RatchetKit"],
            // App code runs on the main thread unless you say otherwise.
            swiftSettings: [.defaultIsolation(MainActor.self)]
        ),
        .testTarget(name: "RatchetKitTests", dependencies: ["RatchetKit"]),
    ]
)
