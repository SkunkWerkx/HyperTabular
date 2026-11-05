<?php

/**
 * Regenerates stubs/hypertabular.stub.php, the IDE and static-analysis stubs for the extension:
 *
 *   php tools/generate-stubs.php [--out <file>]
 *
 * from php/ on Linux or macOS, with cargo-php CARGO_PHP_VERSION installed
 * (`cargo install cargo-php --version <it> --locked`) and a C compiler on PATH. Steps:
 *
 * 1. Builds the extension's debug library, `cargo rustc --crate-type cdylib --features php
 *    --target-dir target/php` in rust/ (on Linux with `-- -C link-arg=-Wl,-z,lazy`, below):
 *    ext-php-rs exports the description cargo-php reads (`ext_php_rs_describe_module_v2`) only
 *    from a debug build. Release builds are untouched.
 * 2. Runs `cargo php stubs --stdout` on it, preloading a library that defines the PHP symbols
 *    the extension references but cargo-php doesn't stand in for (its own list covers
 *    ext-php-rs's bindings only): its data, and on x86-64 Linux its functions too, which Rust
 *    calls through the GOT, so they must resolve when the library loads, `-z lazy` or not.
 * 3. Applies REWRITES (what cargo-php's view of the builders can't show), each of which must
 *    match exactly as often as recorded, and appends VALUE_CLASSES (the classes registered by
 *    hand, which cargo-php can't see).
 *
 * tests/StubTest.php runs this into a temporary file and compares it with the committed stub,
 * and loads the committed stub without the extension to hold it to the extension's surface.
 */

declare(strict_types=1);

// A script, run directly: its helper functions sit beside the steps that use them.
// phpcs:disable PSR1.Files.SideEffects.FoundWithSymbols

const CARGO_PHP_VERSION = '0.2.2';

/**
 * PHP symbols the extension imports beyond cargo-php's stand-ins: data as null pointers,
 * functions as bodies that never run (stub generation calls none of them).
 */
const SYMBOLS_C = <<<'C'
    void *spl_ce_InvalidArgumentException, *spl_ce_LogicException, *spl_ce_RuntimeException;
    void *spl_ce_OutOfBoundsException, *spl_ce_OutOfRangeException, *php_json_serializable_ce;
    void *zend_empty_string, *zend_one_char_string[256], *zend_post_startup_cb;
    int object_init_ex(void) { return -1; }
    void *zend_declare_typed_property(void) { return 0; }
    void zend_update_property(void) {}
    int zend_verify_scalar_type_hint(void) { return 0; }
    C;

/** [search, replace, occurrences]. */
const REWRITES = [
    // ext-php-rs's argument info names a union of classes in one string, which cargo-php
    // prefixes with one backslash.
    ['\\HyperCast\\Success|HyperCast\\Fault', '\\HyperCast\\Success|\\HyperCast\\Fault', 4],
    // `Door::valueSize()` is registered by hand beside the cases (enums.rs), outside
    // cargo-php's view.
    [
        "      case ExcelSerial = 22;\n    }\n",
        "      case ExcelSerial = 22;\n\n"
            . "        /**\n"
            . "         * Bytes one value of this door takes in a column buffer: the core's layout.\n"
            . "         */\n"
            . "        public function valueSize(): int {}\n"
            . "    }\n",
        1,
    ],
    // The classes are registered final; cargo-php's stubs leave the modifier out.
    ["    class Batch implements", "    final class Batch implements", 1],
    ["    class DelimitedReader {", "    final class DelimitedReader {", 1],
    ["    class Header implements", "    final class Header implements", 1],
    ["    class Row {", "    final class Row {", 1],
    ["    class RowIterator implements", "    final class RowIterator implements", 1],
    ["    class Scratch {", "    final class Scratch {", 1],
    ["    class Sheet {", "    final class Sheet {", 1],
    ["    class Tabular {", "    final class Tabular {", 1],
    ["    class Workbook {", "    final class Workbook {", 1],
];

/** values.rs registers these in MINIT as plain PHP classes, outside cargo-php's view. */
const VALUE_CLASSES = <<<'PHP'

    namespace HyperTabular {
        /**
         * How delimited text is split: the separator, whether quotes are honoured, whether the
         * first record is a header, whether blank lines are skipped.
         */
        final readonly class Dialect {
            public function __construct(
                public string $separator,
                public bool $quoting = true,
                public bool $hasHeader = true,
                public bool $skipBlankLines = true,
            ) {}

            /**
             * Comma-separated, quoted, with a header: one instance per request.
             */
            public static function csv(): \HyperTabular\Dialect {}

            /**
             * Tab-separated, quoted, with a header: one instance per request.
             */
            public static function tsv(): \HyperTabular\Dialect {}

            /**
             * Pipe-separated, quoted, with a header: one instance per request.
             */
            public static function psv(): \HyperTabular\Dialect {}
        }

        /**
         * One output column of a plan: which source column it reads, the door it casts through,
         * and the numeric notation. Built with the factory named after its door.
         */
        final readonly class Column {
            private function __construct(
                public int $ordinal,
                public \HyperTabular\Door $door,
                public \HyperCast\NumFormat $format,
                public int $declared = 0,
            ) {}

            /**
             * A boolean column: HyperCast's boolean lexicon, as `bool`.
             */
            public static function bool(int $ordinal): \HyperTabular\Column {}

            /**
             * A signed 8-bit integer column, as `int`.
             */
            public static function i8(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * A signed 16-bit integer column, as `int`.
             */
            public static function i16(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * A signed 32-bit integer column, as `int`.
             */
            public static function i32(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * A signed 64-bit integer column, as `int`.
             */
            public static function i64(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An unsigned 8-bit integer column, as `int`.
             */
            public static function u8(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An unsigned 16-bit integer column, as `int`.
             */
            public static function u16(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An unsigned 32-bit integer column, as `int`.
             */
            public static function u32(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An unsigned 64-bit integer column, as `int`'s two's-complement bit pattern (sprintf('%u')).
             */
            public static function u64(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An IEEE single column, as `float` (widened losslessly).
             */
            public static function f32(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An IEEE double column, as `float`.
             */
            public static function f64(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * An exact decimal column, as HyperCast's `Decimal` triple.
             */
            public static function decimal(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

            /**
             * A UUID column, as the lowercase hyphenated string.
             */
            public static function uuid(int $ordinal): \HyperTabular\Column {}

            /**
             * An RFC 3339 instant column, as a UTC DateTimeImmutable.
             */
            public static function timestamp(int $ordinal): \HyperTabular\Column {}

            /**
             * A Unix-epoch column at the declared precision, as a UTC DateTimeImmutable.
             */
            public static function unix(int $ordinal, \HyperCast\UnixPrecision $precision): \HyperTabular\Column {}

            /**
             * An Excel date-serial column under the declared date system, as a UTC DateTimeImmutable.
             */
            public static function excelSerial(int $ordinal, \HyperCast\ExcelEpoch $epoch): \HyperTabular\Column {}

            /**
             * A strict `yyyy-MM-dd` date column, as a DateTimeImmutable at UTC midnight.
             */
            public static function date(int $ordinal): \HyperTabular\Column {}

            /**
             * A separated-date column under the declared field order, as a DateTimeImmutable at UTC midnight.
             */
            public static function dateOrdered(int $ordinal, \HyperCast\DateOrder $order): \HyperTabular\Column {}

            /**
             * A zone-less civil date-time column under the declared field order, on a UTC-labeled DateTimeImmutable.
             */
            public static function datetime(int $ordinal, \HyperCast\DateOrder $order): \HyperTabular\Column {}

            /**
             * A 24-hour time-of-day column, as an exact `int` of nanoseconds since midnight.
             */
            public static function time(int $ordinal): \HyperTabular\Column {}

            /**
             * A duration column, as HyperCast's `Duration` pair.
             */
            public static function duration(int $ordinal): \HyperTabular\Column {}

            /**
             * A text column: the cell's bytes themselves, untrimmed, quotes resolved.
             */
            public static function text(int $ordinal): \HyperTabular\Column {}
        }

        /**
         * One sheet of a workbook, as `Workbook::sheets()` lists it.
         */
        final class SheetInfo {
            public function __construct(
                public readonly string $name,
                public readonly bool $hidden,
                public readonly string $part,
                public readonly int $index,
            ) {}
        }

        /**
         * How a sheet of a `Workbook` is read.
         */
        final class SheetOptions {
            public function __construct(
                public readonly bool $hasHeader = true,
                public readonly bool $skipEmptyRows = true,
                public readonly int $batchRows = \HyperTabular\DelimitedReader::DEFAULT_BATCH_ROWS,
            ) {}
        }

        /**
         * A structural failure: the input is not rows of cells. Never a cell's verdict.
         */
        final class TabularException extends \RuntimeException {
            public function __construct(
                public readonly \HyperTabular\TabularFailure $kind,
                public readonly int $record,
                public readonly int $recordLine,
                public readonly int $byte,
                public readonly int $expected = 0,
                public readonly int $found = 0,
            ) {}
        }
    }

    PHP;

const HEADER = <<<'PHP'
    <?php

    /**
     * Stubs for the hypertabular extension, for IDEs and static analysis. Generated: don't edit.
     * Regenerate from php/ with `php tools/generate-stubs.php`, which runs
     * `cargo php stubs --stdout` (cargo-php %s) on the debug build of the extension and
     * completes its output (the tool says how).
     */

    PHP;

function fail(string $message): never
{
    fwrite(STDERR, $message . PHP_EOL);
    exit(1);
}

/** Runs `$command` in `$cwd` with `$env` added; returns stdout, or fails with stderr. */
function run(array $command, string $cwd, array $env = []): string
{
    $process = proc_open(
        $command,
        [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
        $pipes,
        $cwd,
        $env === [] ? null : [...getenv(), ...$env]
    );
    if (!is_resource($process)) {
        fail('could not start ' . $command[0]);
    }
    $stdout = (string) stream_get_contents($pipes[1]);
    $stderr = (string) stream_get_contents($pipes[2]);
    fclose($pipes[1]);
    fclose($pipes[2]);
    if (proc_close($process) !== 0) {
        fail(implode(' ', $command) . " failed:\n" . $stderr . $stdout);
    }
    return $stdout;
}

if (PHP_OS_FAMILY === 'Windows') {
    fail('cargo-php generates stubs on Linux and macOS only');
}

$root = dirname(__DIR__);
$rust = dirname($root) . '/rust';
$out = $root . '/stubs/hypertabular.stub.php';
if (($argv[1] ?? null) === '--out' && isset($argv[2])) {
    $out = $argv[2];
} elseif (count($argv) > 1) {
    fail('usage: php tools/generate-stubs.php [--out <file>]');
}

$version = trim(run(['cargo', 'php', '--version'], $rust));
if (!str_starts_with($version, 'cargo-php ' . CARGO_PHP_VERSION . ' ')) {
    fail("cargo-php " . CARGO_PHP_VERSION . " is required (its output is the stub's format), found: {$version}");
}

$darwin = PHP_OS_FAMILY === 'Darwin';
$build = ['cargo', 'rustc', '--quiet', '--crate-type', 'cdylib', '--features', 'php', '--target-dir', 'target/php'];
// Rust links with `-z now` on Linux, which binds every function when the library loads, and
// cargo-php stands in only for ext-php-rs's bindings; `-z lazy` defers the rest, as on macOS.
// The flag reaches only this crate's link, so the cached dependencies are reused.
$linux = $darwin ? [] : ['--', '-C', 'link-arg=-Wl,-z,lazy'];
run([...$build, ...$linux], $rust);
$library = $rust . '/target/php/debug/' . ($darwin ? 'libhypertabular.dylib' : 'libhypertabular.so');

$scratch = sys_get_temp_dir() . '/hypertabular-stubs-' . getmypid();
if (!is_dir($scratch) && !mkdir($scratch, 0700, true)) {
    fail("cannot create {$scratch}");
}
// `fail()` exits, which skips `finally`; a shutdown function runs on every way out.
register_shutdown_function(static function () use ($scratch): void {
    foreach (glob($scratch . '/*') ?: [] as $file) {
        unlink($file);
    }
    if (is_dir($scratch)) {
        rmdir($scratch);
    }
});
$symbols = $scratch . '/symbols' . ($darwin ? '.dylib' : '.so');
file_put_contents($scratch . '/symbols.c', SYMBOLS_C . "\n");
run(['cc', '-shared', '-fPIC', '-o', $symbols, $scratch . '/symbols.c'], $scratch);
$preload = $darwin ? ['DYLD_INSERT_LIBRARIES' => $symbols] : ['LD_PRELOAD' => $symbols];
$stubs = run(['cargo', 'php', 'stubs', '--stdout', $library], $rust, $preload);

$prefix = "<?php\n\n// Stubs for hypertabular\n\n";
if (!str_starts_with($stubs, $prefix)) {
    fail("cargo-php's output no longer starts with the expected lines:\n" . substr($stubs, 0, 200));
}
$stubs = substr($stubs, strlen($prefix));
foreach (REWRITES as [$search, $replace, $expected]) {
    $count = substr_count($stubs, $search);
    if ($count !== $expected) {
        fail("rewrite of '{$search}' expects {$expected} matches, found {$count}");
    }
    $stubs = str_replace($search, $replace, $stubs);
}
$stubs = sprintf(HEADER, CARGO_PHP_VERSION) . "\n" . rtrim($stubs, "\n") . "\n" . VALUE_CLASSES;
if (file_put_contents($out, $stubs) === false) {
    fail("cannot write {$out}");
}
