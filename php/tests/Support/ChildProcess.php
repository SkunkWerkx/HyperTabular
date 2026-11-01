<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

/**
 * Runs code in a child PHP, the only way to observe a process with other extensions loaded, or
 * none at all.
 */
trait ChildProcess
{
    /** Runs `$code` under `PHP_BINARY` with `$phpFlags` and returns its trimmed stdout. */
    private static function child(array $phpFlags, string $code): string
    {
        [$status, $stdout, $stderr] = self::childRun($phpFlags, $code);
        self::assertSame(0, $status, "child failed: {$stdout}{$stderr}");
        return trim($stdout);
    }

    /**
     * Runs `$code` under `PHP_BINARY` with `$phpFlags` and returns its exit status, stdout and
     * stderr, whatever the status.
     *
     * @return array{int, string, string}
     */
    private static function childRun(array $phpFlags, string $code): array
    {
        $process = proc_open(
            [PHP_BINARY, ...$phpFlags, '-r', $code],
            [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
            $pipes
        );
        self::assertIsResource($process);
        $stdout = stream_get_contents($pipes[1]);
        $stderr = stream_get_contents($pipes[2]);
        fclose($pipes[1]);
        fclose($pipes[2]);
        return [proc_close($process), (string) $stdout, (string) $stderr];
    }

    /** This extension's path, as the test run was started with it. */
    private static function ext(): string
    {
        $path = getenv('HYPERTABULAR_PHP_EXTENSION');
        self::assertIsString($path, 'HYPERTABULAR_PHP_EXTENSION is not set; run composer test');
        self::assertNotSame('', $path);
        return $path;
    }

    /**
     * The flag that loads HyperCast's extension into a child started with `-n`: the build the run
     * was started with, or, when this PHP loads one from its ini files, that one, by name.
     */
    private static function hypercast(): string
    {
        $path = getenv('HYPERCAST_PHP_EXTENSION');
        if (\is_string($path) && $path !== '') {
            return 'extension=' . $path;
        }
        $dir = (string) ini_get('extension_dir');
        $file = PHP_OS_FAMILY === 'Windows' ? 'php_hypercast.dll' : 'hypercast.so';
        return 'extension=' . $dir . DIRECTORY_SEPARATOR . $file;
    }
}
