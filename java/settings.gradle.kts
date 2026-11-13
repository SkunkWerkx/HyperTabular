rootProject.name = "hypertabular"

// Development builds against the HyperCast checkout beside this repo, not the published jar: Gradle substitutes
// io.github.skunkwerkx:hypercast with it, whatever version build.gradle.kts asks for. To test against a released
// HyperCast, check out its tag in that checkout. CI (GitHub Actions sets CI=true) always resolves the published
// jar from Maven Central.
val siblingHyperCast = file("../../HyperCast/java")
if (System.getenv("CI") == null && siblingHyperCast.isDirectory) {
    includeBuild(siblingHyperCast)
}

// The smoke program, mirroring csharp/HyperTabular.AotSmokeTest: every native entry point
// the binding declares, crossed once against the real library. `:aot-smoke-test:run` is
// what the forge runs on Alpine; `:aot-smoke-test:nativeRun` is the same program as a
// GraalVM Native Image binary.
include(":aot-smoke-test")

// JMH benchmarks, mirroring rust/benches and csharp/HyperTabular.Benchmarks — run by hand
// with `./gradlew :benchmarks:jmh`.
include(":benchmarks")
