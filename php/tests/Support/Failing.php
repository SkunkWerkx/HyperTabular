<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

// phpcs:disable PSR1.Methods.CamelCapsMethodName.NotCamelCaps -- PHP names a stream wrapper's methods

/**
 * A stream wrapper over `$parts`: each read hands back the next string, or throws the next
 * exception, or fails (`false`).
 */
final class Failing
{
    /** @var resource|null */
    public $context;

    /** @var list<string|\Throwable|false> */
    public static array $parts = [];

    public static function register(): void
    {
        if (!\in_array('hypertabular-failing', stream_get_wrappers(), true)) {
            stream_wrapper_register('hypertabular-failing', self::class);
        }
    }

    public function stream_open(string $path, string $mode, int $options, ?string &$opened): bool
    {
        return true;
    }

    public function stream_read(int $count): string|false
    {
        $part = array_shift(self::$parts) ?? '';
        if ($part instanceof \Throwable) {
            throw $part;
        }
        return $part;
    }

    public function stream_eof(): bool
    {
        return self::$parts === [];
    }

    public function stream_stat(): array
    {
        return [];
    }
}
