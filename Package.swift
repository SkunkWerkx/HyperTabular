// swift-tools-version:6.2
import PackageDescription

// This file exists purely so `.package(url: "https://github.com/SkunkWerkx/HyperTabular", ...)`
// resolves at all — SwiftPM requires Package.swift at the repository root, with no monorepo
// subdirectory support. CI's own build/test still goes through swift/Package.swift
// (working-directory: swift); this one just points its targets' `path:` at the real sources
// instead of duplicating them, and has to stay in step with it — see that file for what
// each target and the dependency is.
let package = Package(
    name: "HyperTabular",
    // macOS 13 floor: kept in sync with swift/Package.swift, whose comment carries the story.
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .library(name: "HyperTabular", targets: ["HyperTabular"])
    ],
    dependencies: [
        .package(url: "https://github.com/SkunkWerkx/HyperCast", from: "0.6.1")
    ],
    targets: [
        .binaryTarget(
            name: "HyperTabularCore",
            path: "swift/HyperTabularCore.artifactbundle"
        ),
        .target(
            name: "HyperTabular",
            dependencies: [
                "HyperTabularCore",
                .product(name: "HyperCast", package: "HyperCast"),
            ],
            path: "swift/Sources/HyperTabular"
        ),
        .testTarget(
            name: "HyperTabularTests",
            dependencies: ["HyperTabular"],
            path: "swift/Tests/HyperTabularTests"
        ),
    ]
)
