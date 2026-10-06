<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperTabular\Tabular;
use PHPUnit\Framework\TestCase;

/**
 * The load probe: which library answered, and that asking never throws.
 */
final class TabularTest extends TestCase
{
    public function testNativeVersionNamesTheLoadedLibrary(): void
    {
        $this->assertSame(self::crateVersion(), Tabular::nativeVersion());
    }

    public function testIsAvailableIsTheNonThrowingProbe(): void
    {
        $this->assertTrue(Tabular::isAvailable());
        // Cached and idempotent — the second answer is the first, no reload.
        $this->assertTrue(Tabular::isAvailable());
    }

    public function testIsAvailableAnswersFalseWhenFfiIsRestricted(): void
    {
        // ffi.enable=0 refuses the FFI API even on the CLI, which is exactly what a
        // restricted web SAPI looks like from inside the binding.
        $this->assertSame('unavailable ffi', self::probe(\dirname(__DIR__) . '/src', '-d', 'ffi.enable=0'));
    }

    public function testIsAvailableAnswersFalseWhenTheExtensionIsMissing(): void
    {
        // -n drops every ini file, and with them a shared ext-ffi. A PHP with ext-ffi
        // compiled in statically has nothing to drop, so there is nothing to prove there.
        $answer = self::probe(\dirname(__DIR__) . '/src', '-n');
        if (str_ends_with($answer, ' ffi')) {
            $this->markTestSkipped('ext-ffi is compiled into this PHP and cannot be unloaded');
        }
        $this->assertSame('unavailable no-ffi', $answer);
    }

    public function testIsAvailableAnswersFalseWhenTheLibraryIsMissing(): void
    {
        // A copy of the binding with no native/ directory beside it, far from any cargo
        // build the development fallback could find.
        $root = sys_get_temp_dir() . '/hypertabular-probe-' . bin2hex(random_bytes(6));
        $src = $root . '/php/src';
        mkdir($src, 0o777, true);
        try {
            foreach (glob(\dirname(__DIR__) . '/src/*.php') as $file) {
                copy($file, $src . '/' . basename($file));
            }
            $this->assertSame('unavailable ffi', self::probe($src));
        } finally {
            array_map('unlink', glob($src . '/*.php'));
            rmdir($src);
            rmdir($root . '/php');
            rmdir($root);
        }
    }

    /**
     * Runs tests/fixtures/probe.php in a child PHP — the only way to observe a process in
     * which the binding cannot load — and returns what it printed.
     */
    private static function probe(string $src, string ...$phpFlags): string
    {
        // Without the dev-loop override, or the probe would load the library it names.
        $env = array_diff_key(getenv(), ['HYPERTABULAR_NATIVE_LIBRARY' => true]);
        $process = proc_open(
            [PHP_BINARY, ...$phpFlags, __DIR__ . '/fixtures/probe.php', $src],
            [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
            $pipes,
            null,
            $env
        );
        self::assertIsResource($process);
        $stdout = stream_get_contents($pipes[1]);
        $stderr = stream_get_contents($pipes[2]);
        self::assertSame(0, proc_close($process), "probe failed: {$stdout}{$stderr}");
        return $stdout;
    }

    /** The crate's own manifest version — walked up from here the way CorpusTest finds corpus/. */
    private static function crateVersion(): string
    {
        $dir = __DIR__;
        // Stop when dirname() stops moving, not at '/': a Windows root is 'C:\\', never '/'.
        for ($parent = \dirname($dir); $parent !== $dir; $dir = $parent, $parent = \dirname($dir)) {
            $candidate = $dir . '/rust/Cargo.toml';
            if (is_file($candidate)) {
                self::assertSame(
                    1,
                    preg_match('/^version\s*=\s*"([^"]+)"/m', file_get_contents($candidate), $match),
                    'rust/Cargo.toml carries no package version'
                );
                return $match[1];
            }
        }
        self::fail('rust/Cargo.toml not found');
    }
}
