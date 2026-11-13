// swift-tools-version:6.2
import PackageDescription

import Foundation

// This file exists purely so `.package(url: "https://github.com/SkunkWerkx/HyperTabular", ...)`
// resolves at all — SwiftPM requires Package.swift at the repository root, with no monorepo
// subdirectory support. CI's own build/test still goes through swift/Package.swift
// (working-directory: swift); this one just points its targets' `path:` at the real sources
// instead of duplicating them, and has to stay in step with it — see that file for what
// each target and the dependency is.

// The core comes from the GitHub Release: two zips per version, the static-library artifact
// bundle (Linux glibc and musl, macOS, Windows, WebAssembly and Android, one archive per
// triple) and, on a Mac, the XCFramework iOS, the iOS simulator and Mac Catalyst link (Xcode
// does not read a static-library artifact bundle). SwiftPM checks each zip against its
// checksum before using it. Release staging (the Forge's stage-release action) writes the
// three lines below for every release, from the zips the release then attaches; none resolves
// before the first release that carries them. HyperCast's package takes its own core the same
// way.
let coreRelease = "1.0.0"
let coreChecksum = ""
let appleCoreChecksum = ""

// In this repository, CI and .github/scripts/local-core.sh write the core into
// swift/HyperTabularCore.artifactbundle/ (gitignored), and then this file links that instead,
// the way swift/Package.swift does: every CI check that builds through this manifest tests the
// core the run built. A consumer's checkout never has one.
func built(_ path: String) -> Bool {
    FileManager.default.fileExists(atPath: "\(Context.packageDirectory)/\(path)")
}
let assets = "https://github.com/SkunkWerkx/HyperTabular/releases/download/v\(coreRelease)"
let localCore = "swift/HyperTabularCore.artifactbundle"
let localAppleCore = "swift/HyperTabularCoreApple.xcframework"
let isLocal = built("\(localCore)/info.json")
#if canImport(Darwin)
    let linksAppleCore = isLocal ? built("\(localAppleCore)/Info.plist") : true
#else
    let linksAppleCore = false
#endif
let coreTargets: [Target] =
    [
        isLocal
            ? .binaryTarget(name: "HyperTabularCore", path: localCore)
            : .binaryTarget(
                name: "HyperTabularCore", url: "\(assets)/HyperTabularCore.artifactbundle.zip",
                checksum: coreChecksum)
    ]
    + (!linksAppleCore
        ? []
        : [
            isLocal
                ? .binaryTarget(name: "HyperTabularCoreApple", path: localAppleCore)
                : .binaryTarget(
                    name: "HyperTabularCoreApple", url: "\(assets)/HyperTabularCoreApple.xcframework.zip",
                    checksum: appleCoreChecksum)
        ])
let coreDependencies: [Target.Dependency] =
    linksAppleCore
    ? [
        .target(name: "HyperTabularCore", condition: .when(platforms: [.macOS, .linux, .windows, .wasi, .android])),
        .target(name: "HyperTabularCoreApple", condition: .when(platforms: [.iOS, .macCatalyst])),
    ] : ["HyperTabularCore"]

// Development builds against the HyperCast checkout beside this one (HyperForge's workspace/, where
// each Hyper* repo builds against its peers' working trees). Never in CI (GitHub Actions sets CI),
// where companion mode rewrites the requirement below to the pinned package and release mode
// resolves it from GitHub, and never in a consumer's build, whose checkouts sit side by side
// under .build/checkouts/ or Xcode's SourcePackages/checkouts/. The path is computed, so the
// Forge's manifest rewriting (companion_wire, companion_gate) sees only the requirement, and its
// release guard and companion wiring fail a leg with a HyperCast checkout beside this one
// (companion_fetch.py, peer_findings). HyperCast links its own local core when it has one: run its
// .github/scripts/local-core.sh as well as this repository's.
let peerHyperCast = "\(Context.packageDirectory)/../HyperCast"
let usesPeerHyperCast =
    Context.environment["CI"] == nil && !Context.packageDirectory.contains("/checkouts/")
    && FileManager.default.fileExists(atPath: "\(peerHyperCast)/Package.swift")

let package = Package(
    name: "HyperTabular",
    // macOS 13, iOS 16 and Mac Catalyst 16 floors: kept in sync with swift/Package.swift,
    // whose comment carries the story.
    platforms: [
        .macOS(.v13),
        .iOS(.v16),
        .macCatalyst(.v16),
    ],
    products: [
        .library(name: "HyperTabular", targets: ["HyperTabular"])
    ],
    dependencies: [
        // 1.0.0, the newest HyperCast release, while 1.1.0 (the real floor) is in development:
        // see rust/Cargo.toml. Its Swift core comes from its release by URL and checksum. A version range,
        // never a revision or a branch: SwiftPM refuses a tagged package that depends on one to
        // every consumer, so the release guard refuses a tag that does.
        usesPeerHyperCast
            ? .package(path: peerHyperCast)
            : .package(url: "https://github.com/SkunkWerkx/HyperCast", from: "1.0.0")
    ],
    targets: [
        .target(
            name: "HyperTabular",
            dependencies: coreDependencies + [
                .product(name: "HyperCast", package: "HyperCast")
            ],
            path: "swift/Sources/HyperTabular"
        ),
        .testTarget(
            name: "HyperTabularTests",
            dependencies: ["HyperTabular"],
            path: "swift/Tests/HyperTabularTests"
        ),
    ] + coreTargets
)
