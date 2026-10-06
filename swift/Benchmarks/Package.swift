// swift-tools-version:5.9
import PackageDescription

// The workbook benchmarks, beside the package rather than in it so that the library's
// consumers never resolve package-benchmark. Run by hand: `swift package benchmark` here
// (HYPERTABULAR_LOCAL_CORE=1 for the core built from this checkout).
let package = Package(
    name: "HyperTabularBenchmarksPackage",
    platforms: [.macOS(.v13)],
    dependencies: [
        .package(path: "../"),
        .package(url: "https://github.com/ordo-one/benchmark", from: "1.4.0"),
    ],
    targets: [
        .executableTarget(
            name: "HyperTabularBenchmarks",
            dependencies: [
                .product(name: "HyperTabular", package: "swift"),
                .product(name: "Benchmark", package: "benchmark"),
            ],
            path: "Benchmarks/HyperTabularBenchmarks",
            plugins: [
                .plugin(name: "BenchmarkPlugin", package: "benchmark")
            ]
        )
    ]
)
