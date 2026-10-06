<?php

declare(strict_types=1);

namespace HyperTabular;

use HyperCast\Interop\NativeValues;

/**
 * The native core this package binds: whether it loaded, and which version answered.
 */
final class Tabular
{
    private static ?bool $available = null;

    /** Static-only facade — never instantiated. */
    private function __construct()
    {
    }

    /**
     * Whether libhypertabular resolved for this platform and is the ABI this binding
     * declares — the probe to gate on instead of catching around the first read. Attempts
     * the same load every reader makes, but never throws: a missing ext-ffi, an
     * `ffi.enable` setting that restricts FFI for this SAPI, a missing or unloadable
     * library, an unsupported platform, or a library of another version all answer false.
     * The answer is cached for the request; true exactly when {@see nativeVersion()}
     * succeeds.
     *
     * @return bool true when a reader can be built; false when building one would throw
     */
    public static function isAvailable(): bool
    {
        if (self::$available !== null) {
            return self::$available;
        }
        try {
            self::nativeVersion();
            return self::$available = true;
        } catch (\Throwable) {
            return self::$available = false;
        }
    }

    /**
     * The version of the native library that actually loaded, as "major.minor.patch" — the
     * core's own manifest version, read through the zero-argument probe it exports, so a
     * host can name a mismatch before the first read. Throws when no library resolves;
     * {@see isAvailable()} is the non-throwing form.
     *
     * @return string the native library's semantic version as "major.minor.patch"
     * @throws \RuntimeException when the native library did not load
     */
    public static function nativeVersion(): string
    {
        return NativeValues::version(Native::ffi()->hypertabular_version());
    }
}
