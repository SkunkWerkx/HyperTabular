<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

// phpcs:disable PSR1.Methods.CamelCapsMethodName.NotCamelCaps -- PHP names a stream wrapper's methods

/** A stream wrapper over strings and exceptions: each read hands back the next, or throws it. */
final class Throwing
{
    /** @var resource|null */
    public $context;

    /** @var list<string|\Throwable> */
    private static array $pending = [];

    /** @var list<string|\Throwable> */
    private array $parts = [];

    public static function register(): void
    {
        if (!\in_array('hypertabular-throwing', stream_get_wrappers(), true)) {
            stream_wrapper_register('hypertabular-throwing', self::class);
        }
    }

    /**
     * @param list<string|\Throwable> $parts
     * @return resource
     */
    public static function open(array $parts)
    {
        self::$pending = $parts;
        return fopen('hypertabular-throwing://stream', 'rb');
    }

    public function stream_open(string $path, string $mode, int $options, ?string &$opened): bool
    {
        $this->parts = self::$pending;
        return true;
    }

    public function stream_read(int $count): string
    {
        $part = array_shift($this->parts) ?? '';
        if ($part instanceof \Throwable) {
            throw $part;
        }
        return $part;
    }

    public function stream_eof(): bool
    {
        return $this->parts === [];
    }
}
