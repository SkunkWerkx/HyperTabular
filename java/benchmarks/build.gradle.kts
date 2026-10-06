// Local-dev-only JMH benchmarks — mirrors rust/benches (Criterion) and
// csharp/HyperTabular.Benchmarks (BenchmarkDotNet): the workbook reader over the two
// 300 000-row files of corpus/README.md. Run by hand with `./gradlew :benchmarks:jmh`.
plugins {
    java
    id("me.champeau.jmh") version "0.7.3"
}

repositories {
    mavenCentral()
}

dependencies {
    implementation(rootProject)
}

java {
    sourceCompatibility = JavaVersion.VERSION_25
    targetCompatibility = JavaVersion.VERSION_25
}

// The library's own floor, enforced the same way it is there.
tasks.withType<JavaCompile>().configureEach {
    options.release = 25
}

jmh {
    // One read of a file is most of a second to two seconds: a handful of iterations of a
    // few seconds each is already dozens of reads, and long enough for the JIT to have
    // settled the batch loop.
    warmupIterations.set(2)
    iterations.set(5)
    fork.set(1)
    warmupForks.set(0)
    timeOnIteration.set("4s")
    warmup.set("4s")
    // Allocation per read is a receipt here, not a claim — gc.alloc.rate.norm is the B/op.
    profilers.set(listOf("gc"))
    // The binding's FFM downcalls are a "restricted method" — the forked JVM needs the same
    // opt-in the library's own test task sets.
    jvmArgsAppend.add("--enable-native-access=ALL-UNNAMED")
    // The corpus directory, for the files; HYPERTABULAR_BENCH_DIR overrides it.
    jvmArgsAppend.add("-Dhypertabular.corpus=" + rootProject.file("../corpus/generate/out").absolutePath)
    // -PjmhInclude=<regex>: a subset of the suite, JMH's own include syntax.
    if (project.hasProperty("jmhInclude")) {
        includes.set(listOf(project.property("jmhInclude") as String))
    }
}
