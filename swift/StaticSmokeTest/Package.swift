// swift-tools-version:6.2
import PackageDescription

// A consumer, not part of the library: an executable that takes HyperTabular the way a real
// project does — through the manifest at the repository root, which brings HyperCast with
// it — and reads a file's worth of text through every door. It exists for the targets
// `swift test` cannot reach. Swift's static Linux SDK (musl) ships no XCTest, so this is
// what proves the linked-in core there; built for glibc or WebAssembly it proves the same
// thing through a consumer's manifest rather than the package's own. It is also the one
// place both cores — libhypertabular and libhypercast — are linked into a real executable.
//
//     swift build --swift-sdk x86_64-swift-linux-musl && .build/debug/StaticSmokeTest
let package = Package(
    name: "StaticSmokeTest",
    platforms: [
        .macOS(.v13)
    ],
    dependencies: [
        .package(name: "HyperTabular", path: "../..")
    ],
    targets: [
        .executableTarget(
            name: "StaticSmokeTest",
            dependencies: [.product(name: "HyperTabular", package: "HyperTabular")],
            path: "Sources"
        )
    ]
)
