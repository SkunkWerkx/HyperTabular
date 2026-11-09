<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

// phpcs:disable PSR1.Methods.CamelCapsMethodName.NotCamelCaps -- PHP names a stream wrapper's methods

/**
 * A stream wrapper that hands back less than it was asked for: one to three bytes a read,
 * whatever the size. A reader has to take a short read for what it is.
 */
final class Dribble
{
    /** @var resource|null */
    public $context;

    /** @var array<int, string> */
    private static array $inputs = [];

    private string $data = '';
    private int $at = 0;
    private int $reads = 0;

    public static function register(): void
    {
        if (!\in_array('hypertabular-dribble', stream_get_wrappers(), true)) {
            stream_wrapper_register('hypertabular-dribble', self::class);
        }
    }

    /** @return resource */
    public static function open(string $input)
    {
        self::$inputs[] = $input;
        return fopen('hypertabular-dribble://' . array_key_last(self::$inputs), 'rb');
    }

    public function stream_open(string $path, string $mode, int $options, ?string &$opened): bool
    {
        $key = (int) substr($path, \strlen('hypertabular-dribble://'));
        $this->data = self::$inputs[$key];
        unset(self::$inputs[$key]);
        return true;
    }

    public function stream_read(int $count): string
    {
        $this->reads++;
        $chunk = substr($this->data, $this->at, min($count, $this->reads % 3 + 1));
        $this->at += \strlen($chunk);
        return $chunk;
    }

    public function stream_eof(): bool
    {
        return $this->at >= \strlen($this->data);
    }
}
