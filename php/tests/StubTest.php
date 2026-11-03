<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperTabular\Tests\Support\ChildProcess;
use HyperTabular\Tests\Support\Surface;
use PHPUnit\Framework\TestCase;

/**
 * stubs/hypertabular.stub.php, the IDE and static-analysis stubs, declares the extension's API
 * exactly, and is what tools/generate-stubs.php makes from the current source.
 */
final class StubTest extends TestCase
{
    use ChildProcess;

    /**
     * Runs everywhere, without cargo-php: the committed stub, loaded alone in a PHP without either
     * extension, has the loaded extension's surface, declaration for declaration. This is what
     * holds the plain classes, which the tool writes by hand, to the extension.
     */
    public function testTheStubDeclaresTheLoadedSurface(): void
    {
        $root = \dirname(__DIR__);
        $stubbed = self::child(
            ['-n'],
            'require ' . var_export($root . '/vendor/autoload.php', true) . ';'
            . 'require ' . var_export($root . '/stubs/hypertabular.stub.php', true) . ';'
            . 'echo json_encode(HyperTabular\Tests\Support\Surface::of(), JSON_THROW_ON_ERROR);'
        );
        // Surface::of() names every HyperTabular class declared, the test suite's own included
        // when it has loaded them; the child loaded none.
        $loaded = array_filter(
            Surface::of(),
            static fn (array $entry, string $name): bool => (new \ReflectionClass($name))->isInternal(),
            ARRAY_FILTER_USE_BOTH
        );
        self::assertSame(
            json_decode(json_encode($loaded, JSON_THROW_ON_ERROR), true, flags: JSON_THROW_ON_ERROR),
            json_decode($stubbed, true, flags: JSON_THROW_ON_ERROR)
        );
    }

    public function testStubIsCurrent(): void
    {
        if (PHP_OS_FAMILY === 'Windows') {
            self::markTestSkipped('cargo-php generates stubs on Linux and macOS only');
        }
        if (getenv('HYPERTABULAR_CHECK_STUBS') !== '1') {
            self::markTestSkipped('set HYPERTABULAR_CHECK_STUBS=1 to regenerate and compare the stubs');
        }
        $root = \dirname(__DIR__);
        $generated = tempnam(sys_get_temp_dir(), 'hypertabular-stub');
        self::assertIsString($generated);
        try {
            $process = proc_open(
                [PHP_BINARY, $root . '/tools/generate-stubs.php', '--out', $generated],
                [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
                $pipes,
                $root
            );
            self::assertIsResource($process);
            $output = stream_get_contents($pipes[1]) . stream_get_contents($pipes[2]);
            fclose($pipes[1]);
            fclose($pipes[2]);
            self::assertSame(0, proc_close($process), "generate-stubs.php failed:\n{$output}");
            self::assertFileEquals($generated, $root . '/stubs/hypertabular.stub.php');
        } finally {
            unlink($generated);
        }
    }
}
