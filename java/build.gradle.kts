import java.util.jar.JarFile
import org.gradle.external.javadoc.StandardJavadocDocletOptions

plugins {
    `java-library`
    `maven-publish`
    id("com.vanniktech.maven.publish") version "0.37.0"
    id("com.diffplug.spotless") version "8.10.4"
}

// io.github.skunkwerkx — the SkunkWerkx org's Central Portal namespace, the one HyperCast
// and HyperUuid publish under.
group = "io.github.skunkwerkx"
// The committed version, which prepare-release.yml bumps (the Forge's hyper_config.py
// matches this line exactly). HYPERTABULAR_VERSION overrides it for a one-off build; nothing
// in CI sets it for Gradle, so the Central bundle release.yml packs always carries the
// committed version. It moves with rust/Cargo.toml: the suite pins the loaded library's own version
// to it.
version = System.getenv("HYPERTABULAR_VERSION") ?: "1.0.0"

repositories {
    mavenCentral()
}

// The one source of the JDK floor: the base of the multi-release jar (every reader and the
// JNI Native) compiles for jdkFloor, and src/main/java25 (the FFM Native) with --release 25.
// Gradle, Spotless, javadoc and publish all run on JDK 25. It is HyperCast's floor too: the
// HyperCast jar this one depends on is the same multi-release shape.
val jdkFloor by extra(21)

// The formatter, over every Java file in the build: the library, its tests, the floor-API
// check (gradle/FloorApiCheck.java), the AOT smoke test and the JMH benchmarks. palantir-java-format is google-java-format's rules at a
// 4-space indent and 120 columns, the shape this code was already written in.
// `./gradlew spotlessApply` formats; `./gradlew spotlessCheck` (CI) fails on any drift.
spotless {
    java {
        target("src/**/*.java", "gradle/*.java", "aot-smoke-test/src/**/*.java", "benchmarks/src/**/*.java")
        palantirJavaFormat("2.102.0")
    }
}

dependencies {
    // HyperCast is the judge: Verdict, Success, Fault, CastFailure, NumFormat,
    // UnixPrecision, DateOrder and ExcelEpoch are its own types, from its own published
    // jar, and they are this binding's public API — hence `api`, so a consumer gets them
    // transitively. Nothing here calls HyperCast's doors: the core this binding loads has
    // HyperCast compiled in, and casts every cell itself. 1.1.0 is the floor: the first
    // HyperCast jar that runs on JDK 21 (a multi-release jar of its own).
    api("io.github.skunkwerkx:hypercast:1.1.0")
    testImplementation(platform("org.junit:junit-bom:6.1.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testImplementation("com.google.code.gson:gson:2.15.0")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

// Local dev loop, mirroring the C# binding's csproj copy of the freshly-built core: when
// the Rust cdylib exists in-repo (`cargo cdylib` in ../rust), stage it as
// the classpath resource /native/{rid}/{lib} the loader expects. CI overlays every
// platform's build into the same layout before packaging.
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
// explicitly on top of that, processResources fails on the duplicate entry (found by
// staging, then placing, in one working tree). Sync removes what it did not stage.
val nativePlaced = file("src/main/resources/native/$nativeRid").exists()
val coreNames = listOf("libhypertabular.so", "libhypertabular.dylib", "hypertabular.dll")
val shimNames = listOf("libhypertabular_jni.so", "libhypertabular_jni.dylib", "hypertabular_jni.dll")
val stageNativeLibrary = tasks.register<Sync>("stageNativeLibrary") {
    from("../rust/target/release") {
        include(coreNames)
        if (nativePlaced || nativeRid == "unsupported") {
            exclude("**")
        }
    }
    // The JNI library beside the core: `cargo build --release --locked --lib` in jni/ (after
    // `cargo cdylib` in ../rust), under the same placed/unsupported exclusion.
    from("jni/target/release") {
        include(shimNames)
        if (nativePlaced || nativeRid == "unsupported") {
            exclude("**")
        }
    }
    into(layout.buildDirectory.dir("generated-resources/native/$nativeRid"))

    // Always runs: the guards below read mtimes, which Gradle's up-to-date check (content
    // hashes of the declared inputs) does not see, so a touched source must still fail here.
    outputs.upToDateWhen { false }
    doFirst {
        if (nativePlaced || nativeRid == "unsupported") return@doFirst
        val coreDir = file("../rust/target/release")
        val shimDir = file("jni/target/release")
        val cores = coreNames.map { File(coreDir, it) }.filter { it.isFile }
        val shims = shimNames.map { File(shimDir, it) }.filter { it.isFile }
        // The pair, or neither (CI placed them, or nothing is built): one without the other
        // is a jar that would load on one JDK and fail on the other.
        if (cores.isNotEmpty() && shims.isEmpty()) {
            throw GradleException(
                "${cores.first().name} is built but the JNI library is not: run `cargo build --release --locked --lib` " +
                    "in java/jni (after `cargo cdylib` in rust/) so the jar carries the pair",
            )
        }
        if (shims.isNotEmpty() && cores.isEmpty()) {
            throw GradleException(
                "${shims.first().name} is built but the core is not: run `cargo cdylib` in rust/ so the jar carries the pair",
            )
        }
        // What cargo itself read: the dep-info file beside each library lists the sources rustc
        // opened, which is exactly what makes cargo rebuild. Walking whole source trees would
        // fail forever on a file the build never reads (python_ext.rs, say). The JNI library holds
        // the core inside it, so its dep-info covers the core's sources too. Dep-info lists no
        // manifests, so the core's Cargo.toml is an extra for both.
        fun depInfo(dep: File): List<File>? {
            if (!dep.isFile) return null
            val line = dep.readLines().firstOrNull { it.contains(": ") } ?: return null
            return line.substringAfter(": ").trim().split(Regex("(?<!\\\\) +"))
                .filter { it.isNotBlank() }.map { File(it.replace("\\ ", " ")) }
        }
        fun stale(lib: File, dep: File, extra: List<File>, remedy: String) {
            val sources = depInfo(dep) ?: throw GradleException("${lib.name}: no dep-info at ${dep.relativeTo(projectDir)}: $remedy")
            val newer = (sources + extra).filter { it.exists() && it.lastModified() > lib.lastModified() }
            if (newer.isNotEmpty()) {
                throw GradleException("${lib.name} is older than ${newer.first().relativeTo(projectDir)}: $remedy")
            }
        }
        val rebuildCore = "run `cargo cdylib` in rust/"
        val rebuildShim = "run `cargo cdylib` in rust/, then `cargo build --release --locked --lib` in java/jni"
        for (core in cores) {
            stale(core, File(coreDir, core.nameWithoutExtension + ".d"), listOf(file("../rust/Cargo.toml")), rebuildCore)
        }
        for (shim in shims) {
            stale(
                shim,
                File(shimDir, shim.nameWithoutExtension + ".d"),
                listOf(file("../rust/Cargo.toml"), file("jni/Cargo.toml"), file("jni/Cargo.lock")),
                rebuildShim,
            )
        }
    }
}

sourceSets.main {
    resources.srcDir(layout.buildDirectory.dir("generated-resources"))
}

// The JDK 25 half of the multi-release jar: the FFM Native, compiled --release 25 against
// main's output (Abi above all) and packed under META-INF/versions/25.
val java25 = sourceSets.create("java25") {
    java.setSrcDirs(listOf("src/main/java25"))
    compileClasspath += sourceSets.main.get().output + configurations.compileClasspath.get()
}

tasks.processResources {
    dependsOn(stageNativeLibrary)
}

// sourcesJar packages the main source set, and `generated-resources` is one of its resource
// dirs (above) — so it reads stageNativeLibrary's output too, and Gradle fails the build
// outright on the undeclared dependency rather than risk a task-order-dependent jar. Only
// the publish path builds sourcesJar (`./gradlew test` never does), which is why this first
// surfaced against Maven Central and not in CI.
//
// withType/configureEach rather than tasks.named("sourcesJar"): the sources and javadoc jars
// are registered by the vanniktech publish plugin, so they don't exist yet at this point in
// configuration and named() fails outright with "Task with name 'sourcesJar' not found".
// configureEach is lazy and picks them up whenever they appear.
tasks.withType<Jar>().configureEach {
    dependsOn(stageNativeLibrary)
}

// Ships the license text and this binding's README inside the jar, under META-INF/ (the
// conventional home for both). Gradle copies from anywhere on disk, so the repo root's
// LICENSE is referenced directly — no local copy, unlike the gem and the wheel, whose
// packers both reject a parent path outright. The POM's <licenses> block stays the
// machine-readable declaration; this is the text itself, for consumers who vendor the jar.
tasks.jar {
    metaInf {
        from("../LICENSE")
        from("README.md")
    }
    // A stable module name for a consumer on the module path, and therefore something
    // exact to hand --enable-native-access. Multi-Release: JDK 25 and later read
    // META-INF/versions/25 (the FFM Native); the base is the JNI Native, which 21 through 24
    // run.
    manifest {
        attributes(
            "Automatic-Module-Name" to "io.github.skunkwerkx.hypertabular",
            "Multi-Release" to "true",
        )
    }
    into("META-INF/versions/25") {
        from(java25.output)
    }
}

// The same contents without the native libraries, for the test that runs with no core on the
// classpath. Never published: nothing attaches it to a publication.
val noNativeJar = tasks.register<Jar>("noNativeJar") {
    archiveClassifier = "nonative"
    from(sourceSets.main.get().output) {
        exclude("native/**")
    }
    into("META-INF/versions/25") {
        from(java25.output)
    }
    manifest {
        attributes(
            "Automatic-Module-Name" to "io.github.skunkwerkx.hypertabular",
            "Multi-Release" to "true",
        )
    }
}

// The sources jar mirrors the layout: the FFM tree under META-INF/versions/25.
tasks.withType<Jar>().matching { it.name == "sourcesJar" }.configureEach {
    into("META-INF/versions/25") {
        from(java25.allJava)
    }
    manifest {
        attributes(
            "Automatic-Module-Name" to "io.github.skunkwerkx.hypertabular",
            "Multi-Release" to "true",
        )
    }
}

// What the multi-release layout must be, checked on the built jar: the JDK's own validator
// (which also holds both trees to one public API), the manifest attribute, the class file
// major versions (65 for the 21 base, 69 under versions/25), a Native in both trees, and both
// libraries in every RID directory. A jar with only the local RID passes;
// staging puts the pair in each one.
val validateMultiReleaseJar = tasks.register("validateMultiReleaseJar") {
    val jarFile = tasks.jar.flatMap { it.archiveFile }
    dependsOn(tasks.jar)
    inputs.file(jarFile)
    doLast {
        val jar = jarFile.get().asFile
        val jarTool = File(System.getProperty("java.home"), "bin/jar" + if (System.getProperty("os.name").lowercase().startsWith("windows")) ".exe" else "")
        val validate = ProcessBuilder(jarTool.path, "--validate", "--file", jar.path).redirectErrorStream(true).start()
        val validateOutput = validate.inputStream.readBytes().decodeToString()
        check(validate.waitFor() == 0) { "jar --validate failed on ${jar.name}:\n$validateOutput" }

        JarFile(jar).use { zip ->
            check(zip.manifest?.mainAttributes?.getValue("Multi-Release") == "true") {
                "${jar.name}: the manifest lacks Multi-Release: true"
            }
            val names = zip.entries().asSequence().map { it.name }.toList()
            for (name in names.filter { it.endsWith(".class") && !it.endsWith("module-info.class") }) {
                val head = zip.getInputStream(zip.getEntry(name)).use { it.readNBytes(8) }
                val major = ((head[6].toInt() and 0xFF) shl 8) or (head[7].toInt() and 0xFF)
                val want = if (name.startsWith("META-INF/versions/25/")) 69 else jdkFloor + 44
                check(major == want) { "${jar.name}: $name is class file major $major, expected $want" }
            }
            val nativeCore = "io/github/skunkwerkx/hypertabular/Native.class"
            for (entry in listOf(nativeCore, "META-INF/versions/25/$nativeCore")) {
                check(entry in names) { "${jar.name}: $entry is missing" }
            }
            val byRid = names.filter { it.startsWith("native/") && !it.endsWith("/") }
                .groupBy({ it.removePrefix("native/").substringBefore('/') }, { it.substringAfterLast('/') })
            for ((rid, files) in byRid) {
                check(files.any { it in coreNames }) { "${jar.name}: native/$rid has no core library ($files)" }
                check(files.any { it in shimNames }) { "${jar.name}: native/$rid has no JNI library ($files)" }
            }
        }
    }
}

java {
    // 21 is the floor, as it is for HyperCast's jar: the oldest long-term-support JDK Temurin
    // still ships for every platform this jar bundles a library for (no JDK 17 for Windows
    // arm64 or Alpine arm64), and it runs the JNI Native; 25 and later take the FFM one from
    // META-INF/versions/25.
    sourceCompatibility = JavaVersion.toVersion(jdkFloor)
    targetCompatibility = JavaVersion.toVersion(jdkFloor)
    withSourcesJar()
}

// The java25 tree compiles --release 25. The base and the tests compile -source/-target
// jdkFloor against JDK 25's own API, not --release jdkFloor: the readers work in native memory
// through java.lang.foreign's memory API on every JDK, and the public API hands out
// MemorySegment views (jar --validate requires one public API across both trees), but
// java.lang.foreign is a preview API in 21's signatures, which javac refuses at --release 21.
// JDK 21 runs it without --enable-preview; only javac draws the line. checkFloorApi below
// takes --release's other job: nothing newer than 21 may be referenced.
tasks.withType<JavaCompile>().configureEach {
    if (name == "compileJava25Java") {
        options.release = 25
    } else {
        // -source 21 without --release warns that the bootstrap class path is not set: on
        // purpose, as above.
        options.compilerArgs.add("-Xlint:-options")
    }
}

// --release's other half, on the floor JDK itself: every class, method and field the base
// and the tests reference in the JDK must exist in jdkFloor with the same descriptor. A 22+
// API (MemorySegment.getString, Arena.allocateFrom) compiles above and fails here, not on a
// consumer's JDK 21.
val checkFloorApi = tasks.register<Exec>("checkFloorApi") {
    val launcher = javaToolchains.launcherFor { languageVersion = JavaLanguageVersion.of(jdkFloor) }
    val classDirs = files(sourceSets.main.get().output.classesDirs, sourceSets.test.get().output.classesDirs)
    val checker = file("gradle/FloorApiCheck.java")
    dependsOn(tasks.compileJava, tasks.compileTestJava)
    inputs.files(classDirs, checker)
    val stamp = layout.buildDirectory.file("floor-api.ok")
    outputs.file(stamp)
    doFirst {
        executable = launcher.get().executablePath.asFile.path
        args(checker.path, *classDirs.files.filter { it.isDirectory }.map { it.path }.toTypedArray())
    }
    doLast { stamp.get().asFile.writeText("ok\n") }
}

// Both suites run the same classes against the jar, which is the unit under test: the
// multi-release layout picks the FFM Native on a 25 JVM and the JNI one on 21.
// `test` runs on Gradle's own JDK (25), `testFloor` on the floor.
fun Test.suite(backend: String, feature: Int) {
    useJUnitPlatform()
    testClassesDirs = sourceSets.test.get().output.classesDirs
    classpath = files(tasks.jar) + sourceSets.test.get().output + configurations.testRuntimeClasspath.get()
    dependsOn(validateMultiReleaseJar, noNativeJar, checkFloorApi)
    inputs.file(noNativeJar.flatMap { it.archiveFile })
    // The no-core child JVM runs against this. Lazy: the task is resolved when the JVM starts,
    // not when the build is configured.
    jvmArgumentProviders.add(
        CommandLineArgumentProvider {
            listOf("-Dhypertabular.noNativeJar=" + noNativeJar.get().archiveFile.get().asFile.absolutePath)
        },
    )
    systemProperty("hypertabular.expectBackend", backend)
    systemProperty("hypertabular.expectFeature", feature.toString())
    // The FFM downcalls are a "restricted method", and on JDK 24+ so is the JNI path's
    // System.load: this silences the runtime warning today and avoids them being
    // blocked outright in a future JDK. -Xcheck:jni makes the VM check every JNI call the
    // library makes; its findings print natively, past Gradle's listeners, so CheckJniTest
    // reads them from a child JVM of its own.
    jvmArgs("--enable-native-access=ALL-UNNAMED", "-Xcheck:jni")
    // This binding's own version, so the suite can pin Tabular.nativeVersion() to it: the core
    // and the jar move together (prepare-release.yml bumps both), and the probe exists to
    // prove exactly that.
    systemProperty("hypertabular.version", version)
}

tasks.test {
    suite("ffm", 25)
}

// The same suite on the floor JDK, through the toolchain: the JNI path end to end.
val testFloor = tasks.register<Test>("testFloor") {
    suite("jni", jdkFloor)
    javaLauncher = javaToolchains.launcherFor {
        languageVersion = JavaLanguageVersion.of(jdkFloor)
    }
}

// Both suites must have run the same tests, none skipped: a test that quietly does not run on
// one JDK is a path nobody checked.
val checkSuiteParity = tasks.register("checkSuiteParity") {
    dependsOn(tasks.test, testFloor)
    val results = layout.buildDirectory.dir("test-results")
    doLast {
        fun read(suite: String): Pair<Set<String>, List<String>> {
            val dir = results.get().dir(suite).asFile
            val files = dir.listFiles { f -> f.name.endsWith(".xml") }.orEmpty()
            check(files.isNotEmpty()) { "no test results in $dir" }
            val ran = sortedSetOf<String>()
            val skipped = mutableListOf<String>()
            for (file in files) {
                val doc = javax.xml.parsers.DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(file)
                val cases = doc.getElementsByTagName("testcase")
                for (i in 0 until cases.length) {
                    val case = cases.item(i) as org.w3c.dom.Element
                    val id = case.getAttribute("classname") + "#" + case.getAttribute("name")
                    ran += id
                    if (case.getElementsByTagName("skipped").length > 0) skipped += id
                }
            }
            return ran to skipped
        }
        val (onTest, skippedTest) = read("test")
        val (onFloor, skippedFloor) = read("testFloor")
        check(skippedTest.isEmpty() && skippedFloor.isEmpty()) {
            "skipped tests: test=$skippedTest testFloor=$skippedFloor"
        }
        check(onTest == onFloor) {
            "the suites differ: only in test=${onTest - onFloor}, only in testFloor=${onFloor - onTest}"
        }
        logger.lifecycle("suite parity: ${onTest.size} tests on each JDK, none skipped")
    }
}

tasks.check {
    dependsOn(testFloor, checkSuiteParity, checkFloorApi)
}

// javadoc's own doclint already flags a missing comment/@param/@return as a WARNING by
// default — -Xwerror promotes those warnings to build-failing errors, so an undocumented
// public member can't ship silently. Central Portal requires a javadoc jar for every
// artifact anyway (see mavenPublishing below), so this is enforcing a real publish
// prerequisite, not just style.
tasks.javadoc {
    (options as StandardJavadocDocletOptions).addBooleanOption("Xwerror", true)
    // The documented source is the 21 base, run on the 25 tool, and documented against 25's
    // API for the reason the compile is: --release 21 refuses java.lang.foreign as preview.
    (options as StandardJavadocDocletOptions).addStringOption("source", jdkFloor.toString())
}

// mavenPublishing {} (com.vanniktech.maven.publish) owns the "maven" publication itself —
// sources/javadoc jars, POM, and the Central Portal repository target all come from here, not
// from a manually created MavenPublication (that would collide: the plugin creates one named
// "maven" too). publishToMavenCentral() targets the new Central Publisher Portal, not the
// dead OSSRH/Nexus staging API. Credentials (mavenCentralUsername/mavenCentralPassword, from
// the Central Portal's own token generator — not a raw Sonatype account password) and the
// signing key come from ORG_GRADLE_PROJECT_-prefixed env vars in CI,
// ~/.gradle/gradle.properties locally; neither lives in this file.
mavenPublishing {
    publishToMavenCentral()
    signAllPublications()

    pom {
        name.set("hypertabular")
        description.set(
            "Forward-only tabular parsing — delimited text and XLSX, ODS and Numbers workbooks read a batch at a time into typed " +
                "columns, every cell a HyperCast Verdict (value or reason + offending span, " +
                "never an exception) — bindings straight into a native Rust core " +
                "(libhypertabular) that never allocates. JDK 21+: JNI on 21–24, FFM on 25+."
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
        // This repo's GitHub Packages Maven registry (private by default, repo-scoped —
        // github.com/SkunkWerkx/HyperTabular/packages). Credentials come from CI's own
        // GITHUB_ACTOR/GITHUB_TOKEN; empty locally, which only matters if you actually run
        // `./gradlew publish` (publishToMavenLocal doesn't touch this repository). Independent
        // of mavenPublishing {} above — this stays as a second, separate target on the same
        // "maven" publication, not a competing one.
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
