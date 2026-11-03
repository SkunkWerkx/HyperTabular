package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.skunkwerkx.hypercast.Cast;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.Verdict;
import java.io.DataInputStream;
import java.io.File;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.jar.Attributes;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.jar.JarOutputStream;
import java.util.jar.Manifest;
import java.util.zip.ZipEntry;
import org.junit.jupiter.api.Test;

/**
 * Which path loaded, and the load-time properties of both: each runs on the floor JDK (JNI) and
 * on 25 (FFM), the expected backend and feature coming from the build. HyperCast's jar, whose
 * types every cell verdict is, rides beside this one and loads a library of its own the moment
 * one of its doors is called; the two share a process on either path.
 */
class BackendTest {
    private static final byte[] CSV = "n\n42\nx\n".getBytes(StandardCharsets.UTF_8);

    private static String expectBackend() {
        return System.getProperty("hypertabular.expectBackend");
    }

    private static Path jarOf(Class<?> type) throws Exception {
        return Path.of(type.getProtectionDomain().getCodeSource().getLocation().toURI());
    }

    private static Path jar() throws Exception {
        return jarOf(Tabular.class);
    }

    /** HyperCast's own jar, the one dependency every child JVM needs beside this one. */
    private static Path hyperCastJar() throws Exception {
        return jarOf(Cast.class);
    }

    private static Path testClasses() throws Exception {
        return jarOf(BackendTest.class);
    }

    private static String classPath(Path... entries) {
        return String.join(
                File.pathSeparator, Arrays.stream(entries).map(Path::toString).toList());
    }

    private static String java() {
        return ProcessHandle.current().info().command().orElse("java");
    }

    /** Runs a child JVM to completion, output merged; fails the test on a hang. */
    private static String run(int expectedExit, List<String> command) throws Exception {
        var log = Files.createTempFile("hypertabular-backend", ".log");
        try {
            var process = new ProcessBuilder(command)
                    .redirectErrorStream(true)
                    .redirectOutput(log.toFile())
                    .start();
            if (!process.waitFor(120, TimeUnit.SECONDS)) {
                process.destroyForcibly();
                throw new AssertionError("child JVM did not finish in 120 s: " + command);
            }
            var output = Files.readString(log, StandardCharsets.UTF_8);
            assertEquals(expectedExit, process.exitValue(), output);
            return output;
        } finally {
            Files.deleteIfExists(log);
        }
    }

    /** The one column of {@link #CSV}, read as i32: a success, then a fault. */
    static List<Verdict<Integer>> readColumn() {
        try (DelimitedReader reader = DelimitedReader.of(CSV, Dialect.CSV, List.of(Column.i32(0)))) {
            Batch batch = reader.read();
            return List.of(batch.get(0, 0, Integer.class), batch.get(0, 1, Integer.class));
        }
    }

    @Test
    void theExpectedPathLoaded() throws Exception {
        assertTrue(Tabular.isAvailable());
        assertEquals(expectBackend(), Native.backend());
        assertEquals(
                Integer.parseInt(System.getProperty("hypertabular.expectFeature")),
                Runtime.version().feature());
        long natives = Arrays.stream(Native.class.getDeclaredMethods())
                .filter(m -> Modifier.isNative(m.getModifiers()))
                .count();
        // One native per C export, and jniNatives, the count the library checks against.
        assertEquals("jni".equals(expectBackend()) ? 15 : 0, natives);
        int major;
        try (InputStream in = Native.class.getResourceAsStream("Native.class")) {
            var data = new DataInputStream(in);
            data.readInt(); // 0xCAFEBABE
            data.readUnsignedShort(); // minor
            major = data.readUnsignedShort();
        }
        assertEquals("jni".equals(expectBackend()) ? 65 : 69, major);
    }

    // HyperCast's library and this one in one JVM, on whichever path this JDK takes: HyperCast's
    // core is compiled into this binding's library with its C exports off, and HyperCast's jar
    // loads its own library for its doors. Each answers alike, and a cell's verdict is the
    // very value HyperCast's door returns for the same text.
    @Test
    void hyperCastsLibraryAndThisOneShareTheProcess() throws Exception {
        for (String first : List.of("hypertabular", "hypercast")) {
            var output = run(
                    0,
                    List.of(
                            java(),
                            "--enable-native-access=ALL-UNNAMED",
                            "-cp",
                            System.getProperty("java.class.path"),
                            BothMain.class.getName(),
                            first));
            assertTrue(
                    output.contains("cells: [Success[value=42], Fault[reason=MALFORMED, offset=0, length=1]]"), output);
            assertTrue(output.contains("doors agree: true"), output);
            assertTrue(output.contains("backends: " + expectBackend() + " " + expectBackend()), output);
        }
    }

    static final class BothMain {
        public static void main(String[] args) throws Exception {
            List<Verdict<Integer>> doors;
            List<Verdict<Integer>> cells;
            if (args[0].equals("hypercast")) {
                doors = List.of(Cast.i32("42", NumFormat.INVARIANT), Cast.i32("x", NumFormat.INVARIANT));
                cells = readColumn();
            } else {
                cells = readColumn();
                doors = List.of(Cast.i32("42", NumFormat.INVARIANT), Cast.i32("x", NumFormat.INVARIANT));
            }
            System.out.println("cells: " + cells);
            System.out.println("doors agree: " + cells.equals(doors));
            Method hyperCast =
                    Class.forName("io.github.skunkwerkx.hypercast.NativeCore").getDeclaredMethod("backend");
            hyperCast.setAccessible(true);
            System.out.println("backends: " + Native.backend() + " " + hyperCast.invoke(null));
        }
    }

    // The attribute stripped, as a shading plugin does: the JDK then reads only the base, so
    // 25 runs the JNI path too. Correct, slower, and its warning names System::load.
    @Test
    void aStrippedJarRunsJniCorrectlyOn25() throws Exception {
        var dir = Files.createTempDirectory("hypertabular-stripped");
        var stripped = dir.resolve("stripped.jar");
        try {
            try (var in = new JarFile(jar().toFile());
                    OutputStream file = Files.newOutputStream(stripped);
                    var out = new JarOutputStream(file)) {
                var manifest = new Manifest(in.getManifest());
                manifest.getMainAttributes().remove(new Attributes.Name("Multi-Release"));
                out.putNextEntry(new ZipEntry(JarFile.MANIFEST_NAME));
                manifest.write(out);
                out.closeEntry();
                for (var entries = in.entries(); entries.hasMoreElements(); ) {
                    JarEntry entry = entries.nextElement();
                    if (entry.getName().equals(JarFile.MANIFEST_NAME)) {
                        continue;
                    }
                    out.putNextEntry(new ZipEntry(entry.getName()));
                    if (!entry.isDirectory()) {
                        try (InputStream body = in.getInputStream(entry)) {
                            body.transferTo(out);
                        }
                    }
                    out.closeEntry();
                }
            }
            // No --enable-native-access: on 24+ the warning is what the README promises.
            var output = run(
                    0,
                    List.of(
                            java(),
                            "-cp",
                            classPath(stripped, hyperCastJar(), testClasses()),
                            StrippedMain.class.getName()));
            assertTrue(output.contains("backend: jni"), output);
            assertTrue(
                    output.contains("cells: [Success[value=42], Fault[reason=MALFORMED, offset=0, length=1]]"), output);
            if (Runtime.version().feature() >= 24) {
                assertTrue(output.contains("java.lang.System::load"), output);
            }
        } finally {
            Files.deleteIfExists(stripped);
            Files.deleteIfExists(dir);
        }
    }

    static final class StrippedMain {
        public static void main(String[] args) {
            System.out.println("backend: " + Native.backend());
            System.out.println("cells: " + readColumn());
        }
    }

    // JNI refuses one library in two class loaders, so each loader extracts its own copy.
    @Test
    void twoClassLoadersEachLoadTheCore() throws Exception {
        URL[] urls = {jar().toUri().toURL(), hyperCastJar().toUri().toURL()};
        try (var a = new URLClassLoader(urls, ClassLoader.getPlatformClassLoader());
                var b = new URLClassLoader(urls, ClassLoader.getPlatformClassLoader())) {
            var versions = new ArrayList<String>();
            for (ClassLoader loader : List.of(a, b)) {
                Class<?> tabular = loader.loadClass(Tabular.class.getName());
                assertFalse(tabular == Tabular.class);
                Method available = tabular.getMethod("isAvailable");
                Method version = tabular.getMethod("nativeVersion");
                assertEquals(true, available.invoke(null));
                versions.add((String) version.invoke(null));
            }
            assertEquals(versions.get(0), versions.get(1));
        }
    }

    @Test
    void theModulePathLoadsTheExpectedBackend() throws Exception {
        var output = run(
                0,
                List.of(
                        java(),
                        "--module-path",
                        classPath(jar(), hyperCastJar()),
                        "--add-modules",
                        "io.github.skunkwerkx.hypertabular",
                        // HyperCast's NativePlatform opens this jar's library on the FFM path,
                        // so its module is named too.
                        "--enable-native-access=io.github.skunkwerkx.hypertabular,io.github.skunkwerkx.hypercast",
                        "-cp",
                        testClasses().toString(),
                        "io.github.skunkwerkx.hypertabular.probe.ModuleMain"));
        assertTrue(output.contains("backend: " + expectBackend()), output);
        assertTrue(output.contains("module: io.github.skunkwerkx.hypertabular"), output);
        assertTrue(output.contains("cell: Success[value=42]"), output);
        assertFalse(output.contains("WARNING"), output);
    }

    @Test
    void withoutACoreTheProbeAnswersFalseAndTheReadersThrowTheLoadFailure() throws Exception {
        // A second JVM with the jar's classes but no native library: the README's contract is
        // that isAvailable() answers false there and never throws, and that a reader throws
        // ExceptionInInitializerError, then NoClassDefFoundError, rather than anything wrapped
        // around them. On either path.
        var output = run(
                0,
                List.of(
                        java(),
                        "--enable-native-access=ALL-UNNAMED",
                        "-cp",
                        classPath(
                                Path.of(System.getProperty("hypertabular.noNativeJar")), hyperCastJar(), testClasses()),
                        NoCore.class.getName()));
        assertTrue(
                output.contains("readers: java.lang.ExceptionInInitializerError java.lang.NoClassDefFoundError"),
                output);
        assertTrue(output.contains("probe: false"), output);
        assertTrue(output.contains("backend: " + expectBackend()), output);
    }

    static final class NoCore {
        private static String reader() {
            try {
                readColumn();
                return "returned";
            } catch (Throwable e) {
                return e.getClass().getName();
            }
        }

        public static void main(String[] args) {
            System.out.println("readers: " + reader() + " " + reader());
            System.out.println("probe: " + Tabular.isAvailable());
            System.out.println("backend: " + Native.backend());
        }
    }

    @Test
    void aRacingFirstTouchLoadsOnce() throws Exception {
        var tmp = Files.createTempDirectory("hypertabular-race");
        try {
            var output = run(
                    0,
                    List.of(
                            java(),
                            "--enable-native-access=ALL-UNNAMED",
                            "-Djava.io.tmpdir=" + tmp,
                            "-cp",
                            System.getProperty("java.class.path"),
                            RaceMain.class.getName()));
            assertTrue(output.contains("all 32 threads succeeded"), output);
            assertTrue(output.contains("extracted: 1"), output);
        } finally {
            try (var left = Files.list(tmp)) {
                for (Path p : (Iterable<Path>) left::iterator) {
                    Files.deleteIfExists(p);
                }
            }
            Files.deleteIfExists(tmp);
        }
    }

    /** 32 threads, one latch, a different first touch on each; then counts the extracted libraries. */
    static final class RaceMain {
        public static void main(String[] args) throws Exception {
            var gate = new CountDownLatch(1);
            var failures = new ConcurrentLinkedQueue<Throwable>();
            List<Runnable> touches = List.of(
                    BackendTest::readColumn,
                    () -> DelimitedReader.of(CSV, Dialect.CSV).close(),
                    () -> {
                        try (Workbook book = Workbook.of("not a zip".getBytes(StandardCharsets.UTF_8))) {
                            throw new AssertionError("opened " + book);
                        } catch (TabularException expected) {
                            // The core answered: a structural error, not a load failure.
                        }
                    },
                    Tabular::nativeVersion,
                    Tabular::isAvailable);
            var threads = new ArrayList<Thread>();
            for (int i = 0; i < 32; i++) {
                Runnable touch = touches.get(i % touches.size());
                Thread t = new Thread(() -> {
                    try {
                        gate.await();
                        touch.run();
                    } catch (Throwable e) {
                        failures.add(e);
                    }
                });
                threads.add(t);
                t.start();
            }
            gate.countDown();
            for (Thread t : threads) {
                t.join();
            }
            if (!failures.isEmpty()) {
                failures.peek().printStackTrace();
                System.exit(1);
            }
            long extracted;
            try (var files = Files.list(Path.of(System.getProperty("java.io.tmpdir")))) {
                extracted = files.filter(p -> p.getFileName().toString().startsWith("hypertabular"))
                        .count();
            }
            System.out.println("all 32 threads succeeded");
            System.out.println("extracted: " + extracted);
        }
    }
}
