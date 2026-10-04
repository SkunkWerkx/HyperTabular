// The smoke program: every native entry point the binding declares, crossed once against
// the real library — mirrors csharp/HyperTabular.AotSmokeTest. `./gradlew
// :aot-smoke-test:run` is the program on an ordinary JVM (what the forge runs on Alpine);
// `./gradlew :aot-smoke-test:nativeRun` under a GraalVM JAVA_HOME is the same program as a
// Native Image binary, which is what proves the FFM downcalls survive ahead-of-time
// compilation.
plugins {
    application
    id("org.graalvm.buildtools.native") version "1.1.10"
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

java {
    sourceCompatibility = JavaVersion.VERSION_25
    targetCompatibility = JavaVersion.VERSION_25
}

// The library's own floor, enforced the same way it is there.
tasks.withType<JavaCompile>().configureEach {
    options.release = 25
}

graalvmNative {
    binaries {
        named("main") {
            // Same restricted-method opt-in as the library's own test task, plus a build
            // report so a failed reachability/linking analysis is diagnosable.
            buildArgs.add("--enable-native-access=ALL-UNNAMED")
            buildArgs.add("-H:+ReportExceptionStackTraces")
            // Deliberately NO `resources { includedPatterns.add("native/.*") }` here: the
            // resource glob and the downcall signatures ship inside the library, in its own
            // reachability-metadata.json, so a consumer inherits them with zero
            // configuration. This program can only prove that by not configuring it itself.
        }
    }
}
