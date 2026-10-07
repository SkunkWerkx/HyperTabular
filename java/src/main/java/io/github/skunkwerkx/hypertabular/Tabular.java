package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativeValues;

/**
 * The native core this jar binds: whether it loaded, which version answered, and by which
 * path. The library rides inside the jar under {@code /native/{rid}/} and is picked by
 * platform at runtime, with the same core as a {@code wasm32-wasip1} module beside it for
 * a platform it has no build for; nothing loads until the first reader is built or this
 * class is asked.
 */
public final class Tabular {
    private Tabular() {}

    /**
     * The system property that picks how the core is reached, read once when it first loads:
     * {@code "native"} for the FFM downcalls into this platform's bundled library, failing
     * when there is none; {@code "wasm"} for the bundled {@code wasm32-wasip1} module run in
     * process by GraalWasm, which the consumer adds ({@code org.graalvm.polyglot:polyglot}
     * and {@code org.graalvm.polyglot:wasm}). Unset, the native library is used when this
     * platform has one that loads, and the module otherwise.
     */
    public static final String BACKEND_PROPERTY = "hypertabular.backend";

    /**
     * Which path this process reaches the core by: {@code "native"} (FFM downcalls into the
     * bundled platform library) or {@code "wasm"} (the bundled module, run by GraalWasm).
     * Decided once, when the core first loads; see {@link #BACKEND_PROPERTY}.
     *
     * @return {@code "native"} or {@code "wasm"}
     */
    public static String backend() {
        return Native.backend();
    }

    /**
     * Whether the core resolved: this platform's library — or, on the wasm path, the module —
     * was found in the jar and loaded, and every export this binding was built against was
     * found in it. Probed once,
     * on first call, and cached; never throws. Gate on this instead of catching the load
     * failure around the first read. {@code true} exactly when {@link #nativeVersion()}
     * succeeds.
     *
     * @return {@code true} when the core is loaded and callable
     */
    public static boolean isAvailable() {
        return Availability.AVAILABLE;
    }

    // Its own holder so the answer is computed once and cached. The probe is the version
    // export: the cheapest crossing there is, and reaching it proves every export resolved.
    private static final class Availability {
        static final boolean AVAILABLE = probe();

        private Availability() {}

        // The one place the binding catches: loading is the caller's environment, not their
        // data. The library failing to load surfaces as ExceptionInInitializerError (and
        // NoClassDefFoundError on every later touch) — LinkageErrors, whatever was
        // underneath. Nothing else is expected, and nothing else is swallowed.
        private static boolean probe() {
            try {
                nativeVersion();
                return true;
            } catch (RuntimeException | LinkageError unavailable) {
                return false;
            }
        }
    }

    /**
     * The version of the native core this process actually loaded, as
     * {@code major.minor.patch}, decoded from the library's own {@code hypertabular_version}
     * export. The probe a host uses to prove the library it resolved is the one this binding
     * was built against, before the first read. The only way it fails is the library not
     * having loaded, which it reports as the load failure itself.
     *
     * @return the loaded core's version as {@code "major.minor.patch"}
     */
    public static String nativeVersion() {
        return NativeValues.version(Native.version());
    }
}
