<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;

/**
 * The native core's C ABI: libhypertabular's six exports and the `#[repr(C)]` shapes that
 * cross them (rust/src/kernel/abi.rs), declared once and bound with PHP's built-in ext-ffi.
 * Every pointer handed over is this binding's own memory for the length of the call; the
 * core keeps nothing.
 *
 * @internal FFI plumbing, not part of the public API — PHP has no package-private visibility.
 */
final class Native
{
    /** The call did what it could; the result says how far it got. */
    public const OK = 0;
    /** A caller bug, not a data verdict. */
    public const ERR_CONTRACT = -1;
    /** The data is structurally broken; the result's failure says where. */
    public const ERR_STRUCTURE = -2;
    /** The arena cannot hold what one row needs; the result says how much that is. */
    public const ERR_ARENA = -3;
    /** The cell table cannot hold one row; the result says how many entries one takes. */
    public const ERR_CELLS = -4;

    /** The flag in the top bit of a span's length; what it means is the field's to say. */
    public const SPAN_FLAG = 0x80000000;
    /** A span's length without its flag. */
    public const SPAN_LENGTH = 0x7FFFFFFF;

    private const DECLARATIONS = <<<'C'
        typedef struct { uint32_t offset; uint32_t len; } ht_span;
        typedef struct { uint32_t offset; uint32_t len; uint32_t reason; } ht_verdict;
        typedef struct {
            uint32_t decimal_sep; uint32_t group_sep; uint32_t flags; uint32_t currency_len;
            uint8_t currency[16];
        } ht_format;
        typedef struct { uint32_t ordinal; uint32_t door; uint32_t param; ht_format format; } ht_column_spec;
        typedef struct { void *values; ht_verdict *verdicts; } ht_column_buffer;
        typedef struct {
            uint32_t code; uint32_t line; uint64_t record; uint64_t byte;
            uint32_t expected; uint32_t found;
        } ht_failure;
        typedef struct {
            uint64_t rows; uint64_t consumed; uint64_t arena_used; uint64_t needed;
            ht_failure failure;
        } ht_filled;
        typedef struct { uint8_t separator; uint8_t quoting; uint8_t skip_blank_lines; uint8_t engine; } ht_dialect;
        typedef struct {
            uint8_t separator; uint8_t quoting; uint8_t skip_blank_lines; uint8_t engine;
            uint32_t line; uint64_t records; uint64_t offset; uint32_t expected;
            uint8_t started; uint8_t reserved[3];
            ht_failure failure;
        } ht_state;
        uint32_t hypertabular_version(void);
        size_t hypertabular_delimited_state_size(void);
        int32_t hypertabular_delimited_init(ht_state *state, const ht_dialect *dialect);
        int32_t hypertabular_delimited_header(
            ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
            ht_span *names, size_t names_cap, uint8_t *arena, size_t arena_cap, ht_filled *out);
        int32_t hypertabular_delimited_fill(
            ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
            const ht_column_spec *specs, const ht_column_buffer *columns, size_t column_count, size_t max_rows,
            ht_span *cells, size_t cells_cap, uint8_t *arena, size_t arena_cap, ht_filled *out);
        size_t hypertabular_delimited_unescape(const char *cell, size_t len, uint8_t *out, size_t cap);
        C;

    private static ?FFI $ffi = null;

    /** Non-instantiable — the bound library is process state. */
    private function __construct()
    {
    }

    /**
     * The bound library, loaded on first use and kept for the request.
     *
     * @return FFI the handle every export is called through
     * @throws \RuntimeException when no library resolves for this platform, or the one that
     *     did is not the ABI this binding declares
     */
    public static function ffi(): FFI
    {
        return self::$ffi ?? self::load();
    }

    /**
     * Resolves the library — the packaged one for this platform, or a development
     * checkout's own cargo build — and binds the declarations to it.
     *
     * @return FFI the bound library handle
     */
    private static function load(): FFI
    {
        if (!\extension_loaded('ffi')) {
            throw new \RuntimeException('hypertabular: the ffi extension is not loaded');
        }
        [$rid, $libName] = NativePlatform::ridAndLibraryName();
        $path = __DIR__ . "/native/{$rid}/{$libName}";
        if (!is_file($path)) {
            // Development loop: fall back to the in-repo cargo build (`cargo cdylib`).
            $repoBuild = \dirname(__DIR__, 2) . "/rust/target/release/{$libName}";
            if (is_file($repoBuild)) {
                $path = $repoBuild;
            }
        }
        if (!is_file($path)) {
            throw new \RuntimeException(
                "hypertabular: {$path} not found (unsupported platform, or this package was built "
                . 'without a native library for it)'
            );
        }

        $ffi = FFI::cdef(self::DECLARATIONS, $path);
        // The state block is the core's to define and this binding's to allocate. A library
        // whose block is another size is another ABI, and is refused before it is handed one.
        $declared = FFI::sizeof($ffi->type('ht_state'));
        $actual = $ffi->hypertabular_delimited_state_size();
        if ($declared !== $actual) {
            throw new \RuntimeException(
                "hypertabular: {$path} has a {$actual}-byte state block where this binding declares "
                . "{$declared} — the library and the package are different versions"
            );
        }
        return self::$ffi = $ffi;
    }
}
