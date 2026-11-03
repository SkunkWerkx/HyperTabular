<?php

declare(strict_types=1);

// The extensions `composer test` and `composer bench` load: this one, the local cargo build
// (`cargo php-ext` in rust/) unless HYPERTABULAR_PHP_EXTENSION names another, which CI does;
// and HyperCast's, which judges every cell: the build HYPERCAST_PHP_EXTENSION names, else one
// this PHP already loads (an install by PIE), else a sibling HyperCast checkout's own `cargo
// php-ext` build. Each is resolved to an absolute path and exported back into its variable, so
// child processes load the same file. Exits with a message when one is missing.

/** The shared library a cargo build of `$crate` writes, on this platform. */
function hypertabular_library(string $crate): string
{
    return match (PHP_OS_FAMILY) {
        'Darwin' => "lib{$crate}.dylib",
        'Windows' => "{$crate}.dll",
        default => "lib{$crate}.so",
    };
}

/** `$path` made absolute and exported as `$variable`, or the run stops with `$hint`. */
function hypertabular_resolve(string $variable, string $path, string $hint): string
{
    $resolved = realpath($path);
    if ($resolved === false || !is_file($resolved)) {
        fwrite(STDERR, "{$path} not found ({$hint})" . PHP_EOL);
        exit(1);
    }
    putenv("{$variable}={$resolved}");
    return $resolved;
}

/**
 * The `-d extension=` flags that give a child PHP both extensions: HyperCast's first (unless
 * this PHP already loads it), then this one.
 *
 * @return list<string>
 */
function hypertabular_extensions(string $root): array
{
    $flags = [];
    $hypercast = getenv('HYPERCAST_PHP_EXTENSION');
    if (\is_string($hypercast) && $hypercast !== '') {
        $flags[] = hypertabular_resolve('HYPERCAST_PHP_EXTENSION', $hypercast, 'HYPERCAST_PHP_EXTENSION');
    } elseif (!\extension_loaded('hypercast')) {
        $flags[] = hypertabular_resolve(
            'HYPERCAST_PHP_EXTENSION',
            \dirname($root, 2) . '/HyperCast/rust/target/php/release/' . hypertabular_library('hypercast'),
            'install HyperCast\'s extension with PIE, run cargo php-ext in a HyperCast checkout beside this '
            . 'one, or name a build in HYPERCAST_PHP_EXTENSION'
        );
    }
    $own = getenv('HYPERTABULAR_PHP_EXTENSION');
    $flags[] = hypertabular_resolve(
        'HYPERTABULAR_PHP_EXTENSION',
        \is_string($own) && $own !== ''
            ? $own
            : $root . '/../rust/target/php/release/' . hypertabular_library('hypertabular'),
        'run cargo php-ext in rust/'
    );
    return $flags;
}

/** Runs $command from $root with this process's standard streams, and exits with its status. */
function hypertabular_run(array $command, string $root, string $what): never
{
    $process = proc_open($command, [0 => STDIN, 1 => STDOUT, 2 => STDERR], $pipes, $root);
    if (!\is_resource($process)) {
        fwrite(STDERR, "could not start {$what}" . PHP_EOL);
        exit(1);
    }
    exit(proc_close($process));
}
