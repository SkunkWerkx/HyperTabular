// swift-tools-version:6.2
import PackageDescription

#if canImport(Darwin)
    import Foundation
#endif

// This file exists purely so `.package(url: "https://github.com/SkunkWerkx/HyperTabular", ...)`
// resolves at all — SwiftPM requires Package.swift at the repository root, with no monorepo
// subdirectory support. CI's own build/test still goes through swift/Package.swift
// (working-directory: swift); this one just points its targets' `path:` at the real sources
// instead of duplicating them, and has to stay in step with it — see that file for what
// each target and the dependency is.
// iOS, the iOS simulator and Mac Catalyst take the core from an XCFramework instead: an app
// for those is built by Xcode, which has linked a static library out of an XCFramework since
// Xcode 12 and does not read a static-library artifact bundle. It carries the same no_std
// archives, one slice each (arm64 only), with the header and module map under
// Headers/HyperTabularCore/, so `import HyperTabularCore` finds the same module either way.
// HyperCast's package does the same for its own core, as of 0.7.0.
//
// Only a Mac can build for those platforms, so only a Mac's manifest declares the target; on
// Linux and Windows this file is what it was. And only when the XCFramework is there: it is
// committed by stage-native-binaries.yml, whole, so a checkout from before its first staging
// has none, and a binary target whose path is missing fails the whole package, macOS
// included.
#if canImport(Darwin)
    let appleCore = "swift/HyperTabularCoreApple.xcframework"
    let linksAppleCore = FileManager.default.fileExists(
        atPath: "\(Context.packageDirectory)/\(appleCore)/Info.plist")
#else
    let appleCore = ""
    let linksAppleCore = false
#endif
let appleCoreTargets: [Target] =
    linksAppleCore ? [.binaryTarget(name: "HyperTabularCoreApple", path: appleCore)] : []
let coreDependency: [Target.Dependency] =
    linksAppleCore
    ? [
        .target(name: "HyperTabularCore", condition: .when(platforms: [.macOS, .linux, .windows, .wasi])),
        .target(name: "HyperTabularCoreApple", condition: .when(platforms: [.iOS, .macCatalyst])),
    ] : ["HyperTabularCore"]

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
        .package(url: "https://github.com/SkunkWerkx/HyperCast", from: "0.7.0")
    ],
    targets: [
        .binaryTarget(
            name: "HyperTabularCore",
            path: "swift/HyperTabularCore.artifactbundle"
        ),
        .target(
            name: "HyperTabular",
            dependencies: coreDependency + [
                .product(name: "HyperCast", package: "HyperCast")
            ],
            path: "swift/Sources/HyperTabular"
        ),
        .testTarget(
            name: "HyperTabularTests",
            dependencies: ["HyperTabular"],
            path: "swift/Tests/HyperTabularTests"
        ),
    ] + appleCoreTargets
)
