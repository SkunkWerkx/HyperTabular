// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "HyperTabular",
    // macOS 13 floor: the duration door presents Swift's own Duration type, as HyperCast's
    // does, and HyperCast's package declares the same floor. Linux builds carry no such
    // availability gate — this only sets the Darwin deployment target.
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .library(name: "HyperTabular", targets: ["HyperTabular"])
    ],
    dependencies: [
        // HyperCast is the judge: every cell is one of its verdicts, and the verdict, the
        // fault, the number format and the declared enums are its own Swift types, from its
        // own package — never copies of them.
        .package(url: "https://github.com/SkunkWerkx/HyperCast", from: "0.6.1")
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
            path: "HyperTabularCore.artifactbundle"
        ),
        .target(
            name: "HyperTabular",
            dependencies: [
                "HyperTabularCore",
                .product(name: "HyperCast", package: "HyperCast"),
            ]
        ),
        .testTarget(
            name: "HyperTabularTests",
            dependencies: ["HyperTabular"]
        ),
    ]
)
