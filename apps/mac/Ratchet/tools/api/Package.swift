// swift-tools-version: 6.2
import PackageDescription

// Only used by tools/generate-api.sh: it pins the version of swift-openapi-generator that wrote Sources/RatchetAPI.
// The app itself never builds this package.
let package = Package(
    name: "RatchetAPIGenerator",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/apple/swift-openapi-generator", exact: "1.14.3")
    ]
)
