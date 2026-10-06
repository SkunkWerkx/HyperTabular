package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativeValues;

/**
 * The native core this jar binds: whether it loaded, and which version answered. The
 * library rides inside the jar under {@code /native/{rid}/} and is picked by platform at
 * runtime; nothing loads until the first reader is built or this class is asked.
 */
public final class Tabular {
    private Tabular() {}

    /**
     * Whether the native library resolved: this platform's build was found in the jar,
     * loaded, and every export this binding was built against was found in it. Probed once,
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
