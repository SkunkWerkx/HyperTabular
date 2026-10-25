<?php

declare(strict_types=1);

// The suite's bootstrap: Composer's autoloader, then the core freshness check. php/src/native
// is committed, and refreshed only when a release stages new binaries, so a plain phpunit
// after a change to rust/ would test the core as it was at the last staging and say nothing
// about it. Before any test runs, this compares the core's inputs (rust/src, rust/Cargo.toml,
// rust/Cargo.lock) at the commit the libraries were built from with the working tree's, and
// stops the run when they differ.
//
// It passes without looking when CI is set (every CI leg builds its own core and copies it
// into php/src/native first), when HYPERTABULAR_NATIVE_LIBRARY names a library (the one
// local-core.sh builds from the checkout), outside a git work tree (the Alpine suite's copy),
// when the staging commit or its source is missing (a shallow clone), and when php/src/native
// itself has working-tree changes other than to a README (someone wrote fresh libraries
// into it).

require __DIR__ . '/../vendor/autoload.php';

(static function (): void {
    $tree = 'php/src/native';
    if ((string) getenv('CI') !== '' || (string) getenv('HYPERTABULAR_NATIVE_LIBRARY') !== '') {
        return;
    }
    $onPath = false;
    foreach (explode(PATH_SEPARATOR, (string) getenv('PATH')) as $dir) {
        foreach (PHP_OS_FAMILY === 'Windows' ? ['git.exe', 'git.cmd', 'git'] : ['git'] as $name) {
            $onPath = $onPath || ($dir !== '' && is_file($dir . DIRECTORY_SEPARATOR . $name));
        }
    }
    if (!$onPath) {
        return;
    }
    // Runs git in $cwd and returns its trimmed output and exit status (-1 when it did not run).
    $git = static function (string $cwd, string ...$args): array {
        $process = proc_open(['git', ...$args], [1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes, $cwd);
        if (!\is_resource($process)) {
            return ['', -1];
        }
        $out = (string) stream_get_contents($pipes[1]);
        stream_get_contents($pipes[2]);
        fclose($pipes[1]);
        fclose($pipes[2]);
        return [trim($out), proc_close($process)];
    };
    [$top, $status] = $git(__DIR__, 'rev-parse', '--show-toplevel');
    if ($status !== 0 || $top === '') {
        return;
    }
    [$staging, $status] = $git($top, 'log', '-1', '--format=%H', '--grep=^chore: stage native binaries', 'HEAD');
    if ($status !== 0 || $staging === '') {
        return;
    }
    [$source] = $git($top, 'log', '-1', '--format=%(trailers:key=Release-Source-SHA,valueonly)', $staging);
    if ($source === '') {
        [$subject] = $git($top, 'log', '-1', '--format=%s', $staging);
        if (preg_match('/\(([0-9a-f]{40})\)$/', $subject, $match) !== 1) {
            return;
        }
        $source = $match[1];
    }
    if ($git($top, 'cat-file', '-e', $source . '^{commit}')[1] !== 0) {
        return;
    }
    $readmes = ":(exclude,glob){$tree}/**/README.md";
    if ($git($top, 'diff', '--quiet', 'HEAD', '--', $tree, $readmes)[1] === 1) {
        return;
    }
    $changed = $git($top, 'diff', '--quiet', $source, '--', 'rust/src', 'rust/Cargo.toml', 'rust/Cargo.lock')[1];
    [$untracked] = $git($top, 'ls-files', '--others', '--exclude-standard', '--', 'rust/src');
    if ($changed !== 1 && $untracked === '') {
        return;
    }
    $library = match (PHP_OS_FAMILY) {
        'Darwin' => 'libhypertabular.dylib',
        'Windows' => 'hypertabular.dll',
        default => 'libhypertabular.so',
    };
    fwrite(
        STDERR,
        "{$tree} was built from rust/ at " . substr($source, 0, 12) . ", and this checkout's rust/ differs "
        . 'from it, so these tests would run against a stale core. Build the checkout\'s own with '
        . '.github/scripts/local-core.sh (FORGE=<path> if the SkunkWerkx/.github checkout is not ../.github '
        . "or ../HyperForge), then run HYPERTABULAR_NATIVE_LIBRARY={$top}/rust/target/release/{$library} "
        . "vendor/bin/phpunit instead; CI builds its own core and skips this check.\n"
    );
    exit(1);
})();
