// Local-dev-only JMH benchmarks — mirrors rust/benches (Criterion) and
// csharp/HyperTabular.Benchmarks (BenchmarkDotNet): the workbook reader over the three
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

val jdkFloor: Int by rootProject.extra

// Compiled for the library's floor the way its base is (-source/-target, not --release: the
// batches are MemorySegment views, a preview API in 21's signatures).
java {
    sourceCompatibility = JavaVersion.toVersion(jdkFloor)
    targetCompatibility = JavaVersion.toVersion(jdkFloor)
}

tasks.withType<JavaCompile>().configureEach {
    options.compilerArgs.add("-Xlint:-options")
}

// The jmh jar bundles the library's classes; without this attribute the JDK reads only the
// jar's base and a 25 JVM silently benchmarks the JNI path.
tasks.named<Jar>("jmhJar") {
    manifest {
        attributes("Multi-Release" to "true")
    }
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
    // The binding's FFM downcalls (and on 24, the JNI path's System.load) are "restricted
    // methods" — the forked JVM needs the same opt-in the library's own test task sets.
    jvmArgsAppend.add("--enable-native-access=ALL-UNNAMED")
    // -PjmhJdk=<feature>: fork the benchmark JVMs on that JDK through the toolchain (the
    // floor, say), the Gradle JVM staying on 25. The default is the Gradle JVM itself.
    if (project.hasProperty("jmhJdk")) {
        val launcher = javaToolchains.launcherFor {
            languageVersion = JavaLanguageVersion.of(project.property("jmhJdk").toString().toInt())
        }
        jvm.set(launcher.map { it.executablePath.asFile.absolutePath })
    }
    // The corpus directory, for the files; HYPERTABULAR_BENCH_DIR overrides it.
    jvmArgsAppend.add("-Dhypertabular.corpus=" + rootProject.file("../corpus/generate/out").absolutePath)
    // -PjmhInclude=<regex>: a subset of the suite, JMH's own include syntax.
    if (project.hasProperty("jmhInclude")) {
        includes.set(listOf(project.property("jmhInclude") as String))
    }
}
