// The smoke program: every native entry point the binding declares, crossed once against
// the real library — mirrors csharp/HyperTabular.AotSmokeTest. `./gradlew
// :aot-smoke-test:run` is the program on an ordinary JVM (what the forge runs on Alpine),
// where JDK 25 takes the jar's FFM path; `:aot-smoke-test:runFloor` the same on the floor JDK,
// where its JNI base loads; and `./gradlew :aot-smoke-test:nativeRun` under a GraalVM
// JAVA_HOME the same program as a Native Image binary, which is what proves the FFM downcalls
// survive ahead-of-time compilation. The image keeps only the FFM path and no JNI library.
plugins {
    application
    id("org.graalvm.buildtools.native") version "1.1.17"
}

repositories {
    mavenCentral()
}

dependencies {
    implementation(rootProject)
}

application {
    mainClass.set("io.github.skunkwerkx.hypertabular.aotsmoketest.Main")
    // The flag a consumer passes for the restricted FFM methods.
    applicationDefaultJvmArgs = listOf("--enable-native-access=ALL-UNNAMED")
}

// The library's own floor, from the root project so the two cannot drift. Gradle and every
// compiler stay on 25 (a JDK 21 javac rejects the library's java25 tree), so only the
// launcher below runs on the floor.
val jdkFloor: Int by rootProject.extra

// Compiled for the floor the way the library's base is, -source/-target rather than
// --release, and for the same reason: this program reads MemorySegment views, and
// java.lang.foreign is a preview API in 21's signatures. runFloor running it on 21 is the
// proof that nothing newer slipped in.
java {
    sourceCompatibility = JavaVersion.toVersion(jdkFloor)
    targetCompatibility = JavaVersion.toVersion(jdkFloor)
}

// The workbook the smoke reads, carried in the program itself so the native binary needs
// no corpus beside it.
tasks.processResources {
    from(rootProject.file("../corpus/workbook/basic.xlsx"))
}

tasks.withType<JavaCompile>().configureEach {
    options.compilerArgs.add("-Xlint:-options")
}

// `run` on the floor JDK, through the toolchain: the jar's JNI base end to end, the same
// launcher as the library's testFloor.
tasks.register<JavaExec>("runFloor") {
    group = "application"
    description = "Runs the smoke test on the floor JDK ($jdkFloor)."
    classpath = sourceSets.main.get().runtimeClasspath
    mainClass = application.mainClass
    jvmArgs("--enable-native-access=ALL-UNNAMED")
    javaLauncher = javaToolchains.launcherFor {
        languageVersion = JavaLanguageVersion.of(jdkFloor)
    }
}

graalvmNative {
    binaries {
        named("main") {
            // Same restricted-method opt-in as the library's own test task, plus a build
            // report so a failed reachability/linking analysis is diagnosable.
            buildArgs.add("--enable-native-access=ALL-UNNAMED")
            buildArgs.add("-H:+ReportExceptionStackTraces")
            // The program's own resource; the library's are its own business, below.
            buildArgs.add("-H:IncludeResources=basic\\.xlsx")
            // Deliberately NO `resources { includedPatterns.add("native/.*") }` here: the
            // resource globs (the core library's three names, never the JNI library's) and
            // the downcall signatures ship inside the library, in its own
            // reachability-metadata.json, so a consumer inherits them with zero
            // configuration. This program can only prove that by not configuring it itself.
        }
    }
}
