// swift-tools-version:6.2
import PackageDescription

#if canImport(Darwin)
    import Foundation
#endif

// The development loop: HYPERTABULAR_LOCAL_CORE=1 links the bundle
// .github/scripts/local-core.sh builds from the checkout, under rust/target/ (which git
// ignores), in place of the committed one, so the suite can run against the core as it stands
// without replacing a committed archive.
let coreBundle =
    Context.environment["HYPERTABULAR_LOCAL_CORE"] == nil
    ? "HyperTabularCore.artifactbundle" : "../rust/target/local-core/swift/HyperTabularCore.artifactbundle"

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
    let appleCore = "HyperTabularCoreApple.xcframework"
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
        .package(url: "https://github.com/SkunkWerkx/HyperCast", from: "0.7.0")
    ],
    targets: [
        // The native core as static libraries, one per triple (SE-0482, which is what sets
        // the tools version above): glibc and musl Linux and macOS on x86_64 and arm64,
        // Windows (MSVC) on x86_64 and arm64, and WASI. SwiftPM picks the variant for the
        // triple being built and links it into the consumer's executable, so nothing ships
        // beside it and nothing is opened at run time. A triple with no variant has no
        // `HyperTabularCore` module, and the build stops there rather than at run time.
        .binaryTarget(
            name: "HyperTabularCore",
            path: coreBundle
        ),
        .target(
            name: "HyperTabular",
            dependencies: coreDependency + [
                .product(name: "HyperCast", package: "HyperCast")
            ]
        ),
        .testTarget(
            name: "HyperTabularTests",
            dependencies: ["HyperTabular"]
        ),
    ] + appleCoreTargets
)
