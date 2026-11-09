<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

// phpcs:disable PSR1.Methods.CamelCapsMethodName.NotCamelCaps -- PHP names a stream wrapper's methods

/**
 * A stream wrapper over `$transfers`: one a read, and then, until `$released`, a read is a
 * failure of the test that made it.
 */
final class Stalling
{
    /** @var resource|null */
    public $context;

    public static int $reads = 0;
    public static bool $released = false;

    /** @var list<string> */
    private static array $pending = [];

    /** @var list<string> */
    private array $transfers = [];

    public static function register(): void
    {
        if (!\in_array('hypertabular-stalling', stream_get_wrappers(), true)) {
            stream_wrapper_register('hypertabular-stalling', self::class);
        }
    }

    /**
     * @param list<string> $transfers
     * @return resource
     */
    public static function open(array $transfers)
    {
        self::$pending = $transfers;
        self::$reads = 0;
        self::$released = false;
        return fopen('hypertabular-stalling://stream', 'rb');
    }

    public function stream_open(string $path, string $mode, int $options, ?string &$opened): bool
    {
        $this->transfers = self::$pending;
        return true;
    }

    public function stream_read(int $count): string
    {
        self::$reads++;
        if ($this->transfers !== []) {
            return array_shift($this->transfers);
        }
        if (!self::$released) {
            throw new \LogicException('it waited on the stream with finished rows in hand');
        }
        return '';
    }

    public function stream_eof(): bool
    {
        return $this->transfers === [] && self::$released;
    }
}
