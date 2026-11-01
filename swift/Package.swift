// swift-tools-version:6.2
import PackageDescription

import Foundation

// The native core is a static-library artifact bundle, one archive per triple (SE-0482, which
// is what sets the tools version above): glibc and musl Linux and macOS on x86_64 and arm64,
// Windows (MSVC) on x86_64 and arm64, Android on x86_64 and arm64, and WASI. SwiftPM picks the
// variant for the triple being built and links it into the consumer's executable, so nothing
// ships beside it and nothing is opened at run time. A triple with no variant has no
// `HyperTabularCore` module, and the build stops there rather than at run time.
//
// The core is build output, never committed: CI writes each leg's archives (and info.json)
// into HyperTabularCore.artifactbundle/ before any Swift step, and
// .github/scripts/local-core.sh writes the host's. The bundle is declared only once its
// info.json is there, so the package still resolves without one (Dependabot resolves
// swift/Benchmarks through this file); a build then stops at `import HyperTabularCore`.
// Consumers never see this file: they resolve the root Package.swift, which takes the core
// from the release's assets.
//
// iOS, the iOS simulator and Mac Catalyst take the core from an XCFramework instead: an app
// for those is built by Xcode, which has linked a static library out of an XCFramework since
// Xcode 12 and does not read a static-library artifact bundle. It carries the same no_std
// archives, one slice each (arm64 only), with the header and module map under
// Headers/HyperTabularCore/, so `import HyperTabularCore` finds the same module either way.
// Only a Mac builds one (the full static-libs build), so it is declared only when it is there.
// HyperCast's package does the same for its own core.
func built(_ path: String) -> Bool {
    FileManager.default.fileExists(atPath: "\(Context.packageDirectory)/\(path)")
}
let core = "HyperTabularCore.artifactbundle"
let appleCore = "HyperTabularCoreApple.xcframework"
let linksCore = built("\(core)/info.json")
let linksAppleCore = built("\(appleCore)/Info.plist")
let coreTargets: [Target] =
    (linksCore ? [.binaryTarget(name: "HyperTabularCore", path: core)] : [])
    + (linksAppleCore ? [.binaryTarget(name: "HyperTabularCoreApple", path: appleCore)] : [])
let coreDependencies: [Target.Dependency] =
    (linksCore
        ? [.target(name: "HyperTabularCore", condition: .when(platforms: [.macOS, .linux, .windows, .wasi, .android]))]
        : [])
    + (linksAppleCore
        ? [.target(name: "HyperTabularCoreApple", condition: .when(platforms: [.iOS, .macCatalyst]))] : [])

let package = Package(
    name: "HyperTabular",
    // macOS 13 floor: the duration door presents Swift's own Duration type, as HyperCast's
    // does, and HyperCast's package declares the same floor; iOS and Mac Catalyst 16 for the
    // same reason. Linux builds carry no such availability gate — this only sets the Darwin
    // deployment targets.
    platforms: [
        .macOS(.v13),
        .iOS(.v16),
        .macCatalyst(.v16),
    ],
    products: [
        .library(name: "HyperTabular", targets: ["HyperTabular"])
    ],
    dependencies: [
        // HyperCast is the judge: every cell is one of its verdicts, and the verdict, the
        // fault, the number format and the declared enums are its own Swift types, from its
        // own package — never copies of them.
        // 1.1.0, whose Swift core comes from its release by URL and checksum. A version range,
        // never a revision or a branch: SwiftPM refuses a tagged package that depends on one to
        // every consumer, so the release guard refuses a tag that does.
        .package(url: "https://github.com/SkunkWerkx/HyperCast", from: "1.1.0")
    ],
    targets: [
        .target(
            name: "HyperTabular",
            dependencies: coreDependencies + [
                .product(name: "HyperCast", package: "HyperCast")
            ]
        ),
        .testTarget(
            name: "HyperTabularTests",
            dependencies: ["HyperTabular"]
        ),
    ] + coreTargets
)
