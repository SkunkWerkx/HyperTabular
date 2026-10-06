import org.gradle.external.javadoc.StandardJavadocDocletOptions

plugins {
    `java-library`
    `maven-publish`
    id("com.vanniktech.maven.publish") version "0.37.0"
    id("com.diffplug.spotless") version "8.10.3"
}

// io.github.skunkwerkx — the SkunkWerkx org's Central Portal namespace, the one HyperCast
// and HyperUuid publish under.
group = "io.github.skunkwerkx"
// CI may override this (0.1.0-ci.<run_number>) via HYPERTABULAR_VERSION so repeated manual
// runs do not collide with a published version; a real publish never sets it and uses the
// committed version as-is. It moves with rust/Cargo.toml: the suite pins the loaded
// library's own version to it.
version = System.getenv("HYPERTABULAR_VERSION") ?: "0.1.0"

repositories {
    mavenCentral()
}

// The formatter, over every Java file in the build: the library, its tests, the AOT smoke
// test and the benchmarks. palantir-java-format is google-java-format's rules at a 4-space
// indent and 120 columns, the shape this code was already written in.
// `./gradlew spotlessApply` formats; `./gradlew spotlessCheck` (CI) fails on any drift.
spotless {
    java {
        target("src/**/*.java", "aot-smoke-test/src/**/*.java", "benchmarks/src/**/*.java")
        palantirJavaFormat("2.102.0")
    }
}

dependencies {
    // HyperCast is the judge: Verdict, Success, Fault, CastFailure, NumFormat,
    // UnixPrecision, DateOrder and ExcelEpoch are its own types, from its own published
    // jar, and they are this binding's public API — hence `api`, so a consumer gets them
    // transitively. Nothing here calls HyperCast's doors: the core this binding loads has
    // HyperCast compiled in, and casts every cell itself.
    api("io.github.skunkwerkx:hypercast:0.6.2")
    testImplementation(platform("org.junit:junit-bom:6.1.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testImplementation("com.google.code.gson:gson:2.11.0")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

// Local dev loop, mirroring the C# binding's csproj copy of the freshly-built core: when
// the Rust cdylib exists in-repo (`cargo cdylib` in ../rust), stage it as the classpath
// resource /native/{rid}/{lib} the loader expects. CI places every platform's build into
// the same layout under src/main/resources before packaging.
//
// The same resolution NativePlatform.java does at runtime, so the library lands under the
// RID the loader will ask for: x64 or arm64 only, and on Linux the musl family when this
// (Gradle's own) JVM has musl's loader mapped — `cargo cdylib` on Alpine produces a musl
// library, and it has to be staged as linux-musl-*. Anything else stages nothing.
val nativeRid = run {
    val osName = System.getProperty("os.name").lowercase()
    val arch = when (System.getProperty("os.arch").lowercase()) {
        "amd64", "x86_64", "x64" -> "x64"
        "aarch64", "arm64" -> "arm64"
        else -> null
    }
    val musl = runCatching {
        file("/proc/self/maps").readLines(Charsets.ISO_8859_1)
            .any { it.contains("ld-musl-") || it.contains("libc.musl-") }
    }.getOrDefault(false)
    when {
        arch == null -> "unsupported"
        osName.startsWith("windows") -> "win-$arch"
        osName.startsWith("mac") || osName.startsWith("darwin") -> "osx-$arch"
        osName.startsWith("linux") -> if (musl) "linux-musl-$arch" else "linux-$arch"
        else -> "unsupported"
    }
}

// Only when this platform's library has NOT been placed under src/main/resources
// explicitly (the forge's java_resources_dir, and the Alpine suite): explicit placement is
// the signal that the right bytes are already on the classpath, and whatever else sits in
// rust/target/release by then is not to be staged beside them.
//
// A Sync that stages nothing, rather than a Copy that is skipped: a skipped task leaves
// whatever it staged last time in generated-resources, and the moment a library is placed
// explicitly on top of that, processResources fails on the duplicate entry. Sync removes
// what it did not stage.
val nativePlaced = file("src/main/resources/native/$nativeRid").exists()
val stageNativeLibrary = tasks.register<Sync>("stageNativeLibrary") {
    from("../rust/target/release") {
        include("libhypertabular.so", "libhypertabular.dylib", "hypertabular.dll")
        if (nativePlaced || nativeRid == "unsupported") {
            exclude("**")
        }
    }
    into(layout.buildDirectory.dir("generated-resources/native/$nativeRid"))
}

sourceSets.main {
    resources.srcDir(layout.buildDirectory.dir("generated-resources"))
}

tasks.processResources {
    dependsOn(stageNativeLibrary)
}

// sourcesJar packages the main source set, and `generated-resources` is one of its resource
// dirs (above) — so it reads stageNativeLibrary's output too, and Gradle fails the build
// outright on the undeclared dependency. withType/configureEach rather than
// tasks.named("sourcesJar"): the sources and javadoc jars are registered by the publish
// plugin, so they do not exist yet at this point in configuration.
tasks.withType<Jar>().configureEach {
    dependsOn(stageNativeLibrary)
}

// Ships the license text and this binding's README inside the jar, under META-INF/.
tasks.jar {
    metaInf {
        from("../LICENSE")
        from("README.md")
    }
    // A stable module name for a consumer on the module path, and therefore something
    // exact to hand --enable-native-access.
    manifest {
        attributes("Automatic-Module-Name" to "io.github.skunkwerkx.hypertabular")
    }
}

tasks.test {
    useJUnitPlatform()
    // The FFM downcalls are a "restricted method" — silences the runtime warning today and
    // avoids them being blocked outright in a future JDK.
    jvmArgs("--enable-native-access=ALL-UNNAMED")
    // This binding's own version, so the suite can pin Tabular.nativeVersion() to it: the
    // core and the jar move together, and the probe exists to prove exactly that.
    systemProperty("hypertabular.version", version)
}

java {
    // 25 is the floor, as it is for HyperCast's jar, which this one depends on: the first
    // long-term-support JDK with the final java.lang.foreign API.
    sourceCompatibility = JavaVersion.VERSION_25
    targetCompatibility = JavaVersion.VERSION_25
    withSourcesJar()
}

// --release, not just the -source/-target pair above: it also compiles against JDK 25's own
// API signatures whatever JDK is running the build.
tasks.withType<JavaCompile>().configureEach {
    options.release = 25
}

// -Xwerror promotes javadoc's own doclint warnings — a missing comment, @param or @return —
// to build-failing errors, so an undocumented public member cannot ship silently. The
// forge runs `javadoc` beside `test` on every leg.
tasks.javadoc {
    (options as StandardJavadocDocletOptions).addBooleanOption("Xwerror", true)
}

// mavenPublishing {} (com.vanniktech.maven.publish) owns the "maven" publication itself —
// sources/javadoc jars, POM, and the Central Portal repository target. Credentials and the
// signing key come from ORG_GRADLE_PROJECT_-prefixed env vars in CI; neither lives here.
mavenPublishing {
    publishToMavenCentral()
    signAllPublications()

    pom {
        name.set("hypertabular")
        description.set(
            "Forward-only tabular parsing — delimited text and XLSX/ODS workbooks read a batch at a time into typed " +
                "columns, every cell a HyperCast Verdict (value or reason + offending span, " +
                "never an exception) — over FFM bindings straight into a native Rust core " +
                "(libhypertabular) that never allocates. JDK 25+."
        )
        url.set("https://github.com/SkunkWerkx/HyperTabular")
        licenses {
            license {
                name.set("MIT")
                url.set("https://opensource.org/license/mit")
                distribution.set("repo")
            }
        }
        developers {
            developer {
                id.set("buvinghausen")
                name.set("Brian Buvinghausen")
                url.set("https://github.com/buvinghausen/")
            }
        }
        scm {
            url.set("https://github.com/SkunkWerkx/HyperTabular")
            connection.set("scm:git:git://github.com/SkunkWerkx/HyperTabular.git")
            developerConnection.set("scm:git:ssh://git@github.com/SkunkWerkx/HyperTabular.git")
        }
    }
}

publishing {
    repositories {
        // This repo's GitHub Packages Maven registry. Credentials come from CI's own
        // GITHUB_ACTOR/GITHUB_TOKEN; empty locally, which only matters to `./gradlew publish`.
        maven {
            name = "GitHubPackages"
            url = uri("https://maven.pkg.github.com/SkunkWerkx/HyperTabular")
            credentials {
                username = System.getenv("GITHUB_ACTOR")
                password = System.getenv("GITHUB_TOKEN")
            }
        }
    }
}
