rootProject.name = "hypertabular"

// The smoke program, mirroring csharp/HyperTabular.AotSmokeTest: every native entry point
// the binding declares, crossed once against the real library. `:aot-smoke-test:run` is
// what the forge runs on Alpine; `:aot-smoke-test:nativeRun` is the same program as a
// GraalVM Native Image binary.
include(":aot-smoke-test")

// JMH benchmarks, mirroring rust/benches and csharp/HyperTabular.Benchmarks — run by hand
// with `./gradlew :benchmarks:jmh`.
include(":benchmarks")
