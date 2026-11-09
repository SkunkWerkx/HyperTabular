<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

// phpcs:disable PSR1.Methods.CamelCapsMethodName.NotCamelCaps -- PHP names a stream wrapper's methods

/**
 * A stream wrapper over `$parts`, one a read, that calls `$callback` from inside its `$at`th read:
 * the caller's PHP running mid-read, as a wrapper's can.
 */
final class Calling
{
    /** @var resource|null */
    public $context;

    /** @var list<string> */
    private static array $pending = [];

    /** @var (callable(): void)|null */
    private static $callback = null;

    private static int $at = 0;

    /** @var list<string> */
    private array $parts = [];

    private int $reads = 0;

    /**
     * @param list<string> $parts
     * @param callable(): void $callback
     * @return resource
     */
    public static function open(array $parts, int $at, callable $callback)
    {
        if (!\in_array('hypertabular-calling', stream_get_wrappers(), true)) {
            stream_wrapper_register('hypertabular-calling', self::class);
        }
        self::$pending = $parts;
        self::$at = $at;
        self::$callback = $callback;
        return fopen('hypertabular-calling://stream', 'rb');
    }

    public function stream_open(string $path, string $mode, int $options, ?string &$opened): bool
    {
        $this->parts = self::$pending;
        return true;
    }

    public function stream_read(int $count): string
    {
        $this->reads++;
        if ($this->reads === self::$at && self::$callback !== null) {
            $callback = self::$callback;
            self::$callback = null;
            $callback();
        }
        return array_shift($this->parts) ?? '';
    }

    public function stream_eof(): bool
    {
        return $this->parts === [];
    }
}
