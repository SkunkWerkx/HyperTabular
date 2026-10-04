<?php

declare(strict_types=1);

namespace HyperTabular;

use HyperCast\DateOrder;
use HyperCast\ExcelEpoch;
use HyperCast\NumFormat;
use HyperCast\UnixPrecision;

/**
 * One output column of a plan: which source column it reads, the door it casts through,
 * and — for the numeric doors — the notation. A plan is a projection: a forty-column file
 * can be read into five typed columns, in any order, and a source column can be read
 * through more than one door.
 *
 * Built with the factory named after its door — {@see Column::i32()}, {@see Column::text()}
 * and so on, the names HyperCast's `Cast` gives the same doors. What a door declares
 * beside itself (a Unix precision, a date order, an Excel date system) is HyperCast's own
 * enum, and the numeric notation is HyperCast's own `NumFormat`; a numeric factory given
 * none reads the invariant notation.
 */
final readonly class Column
{
    /** The widest ordinal a plan can name: the core counts cells per record in 32 bits. */
    private const MAX_ORDINAL = 0x7FFFFFFE;

    /**
     * Carries the column as declared; the factories are the way in.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param Door $door the door the column is cast through
     * @param NumFormat $format the numeric notation, read by the numeric doors
     * @param int $declared what the door declares beside itself, as the core numbers it
     * @throws \InvalidArgumentException when the ordinal is negative or beyond the core's reach
     */
    private function __construct(
        public int $ordinal,
        public Door $door,
        public NumFormat $format,
        public int $declared = 0,
    ) {
        if ($ordinal < 0 || $ordinal > self::MAX_ORDINAL) {
            throw new \InvalidArgumentException(
                "A column's ordinal must be between 0 and " . self::MAX_ORDINAL . "; got {$ordinal}"
            );
        }
    }

    /**
     * A boolean column: HyperCast's boolean lexicon, as `bool`.
     *
     * @param int $ordinal zero-based ordinal of the source column; past a record's last cell reads as empty
     * @return self the column
     */
    public static function bool(int $ordinal): self
    {
        return new self($ordinal, Door::Bool, NumFormat::invariant());
    }

    /**
     * A signed 8-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function i8(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::I8, $format ?? NumFormat::invariant());
    }

    /**
     * A signed 16-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function i16(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::I16, $format ?? NumFormat::invariant());
    }

    /**
     * A signed 32-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function i32(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::I32, $format ?? NumFormat::invariant());
    }

    /**
     * A signed 64-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function i64(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::I64, $format ?? NumFormat::invariant());
    }

    /**
     * An unsigned 8-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function u8(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::U8, $format ?? NumFormat::invariant());
    }

    /**
     * An unsigned 16-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function u16(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::U16, $format ?? NumFormat::invariant());
    }

    /**
     * An unsigned 32-bit integer column, as `int`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function u32(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::U32, $format ?? NumFormat::invariant());
    }

    /**
     * An unsigned 64-bit integer column. PHP has no unsigned 64, so a value comes back as
     * `int`'s two's-complement bit pattern, exactly as HyperCast's `Cast::u64()` presents
     * it — render with sprintf('%u', ...).
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function u64(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::U64, $format ?? NumFormat::invariant());
    }

    /**
     * An IEEE single column, as `float` (widened losslessly).
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function f32(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::F32, $format ?? NumFormat::invariant());
    }

    /**
     * An IEEE double column, as `float`.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function f64(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::F64, $format ?? NumFormat::invariant());
    }

    /**
     * An exact decimal column, as HyperCast's {@see \HyperCast\Decimal} triple. Never a
     * float, and never rounded.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param NumFormat|null $format the declared numeric notation; null is the invariant one
     * @return self the column
     */
    public static function decimal(int $ordinal, ?NumFormat $format = null): self
    {
        return new self($ordinal, Door::Decimal, $format ?? NumFormat::invariant());
    }

    /**
     * A UUID column, as the lowercase hyphenated string.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @return self the column
     */
    public static function uuid(int $ordinal): self
    {
        return new self($ordinal, Door::Uuid, NumFormat::invariant());
    }

    /**
     * An RFC 3339 instant column, as a UTC DateTimeImmutable. Sub-microsecond nanoseconds
     * truncate (PHP's ceiling).
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @return self the column
     */
    public static function timestamp(int $ordinal): self
    {
        return new self($ordinal, Door::Timestamp, NumFormat::invariant());
    }

    /**
     * A Unix-epoch column at the declared precision — never guessed from magnitude — as a
     * UTC DateTimeImmutable.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param UnixPrecision $precision the declared unit of the epoch value
     * @return self the column
     */
    public static function unix(int $ordinal, UnixPrecision $precision): self
    {
        return new self($ordinal, Door::Unix, NumFormat::invariant(), $precision->value);
    }

    /**
     * An Excel date-serial column under the declared date system, as a UTC
     * DateTimeImmutable.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param ExcelEpoch $epoch the declared date system
     * @return self the column
     */
    public static function excelSerial(int $ordinal, ExcelEpoch $epoch): self
    {
        return new self($ordinal, Door::ExcelSerial, NumFormat::invariant(), $epoch->value);
    }

    /**
     * A strict `yyyy-MM-dd` date column, as a DateTimeImmutable at UTC midnight (PHP has no
     * date-only type).
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @return self the column
     */
    public static function date(int $ordinal): self
    {
        return new self($ordinal, Door::Date, NumFormat::invariant());
    }

    /**
     * A separated-date column under the declared field order — "1/7/2026" is January 7th or
     * July 1st only because the caller said which — as a DateTimeImmutable at UTC midnight.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param DateOrder $order the declared field order
     * @return self the column
     */
    public static function dateOrdered(int $ordinal, DateOrder $order): self
    {
        return new self($ordinal, Door::DateOrdered, NumFormat::invariant(), $order->value);
    }

    /**
     * A zone-less civil date-time column under the declared field order. PHP has no
     * zone-less datetime type, so the civil value rides a UTC-labeled DateTimeImmutable —
     * the label is a carrier artifact: no zone was read and none was applied.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @param DateOrder $order the declared field order
     * @return self the column
     */
    public static function datetime(int $ordinal, DateOrder $order): self
    {
        return new self($ordinal, Door::DateTime, NumFormat::invariant(), $order->value);
    }

    /**
     * A 24-hour time-of-day column, as an exact `int` of nanoseconds since midnight.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @return self the column
     */
    public static function time(int $ordinal): self
    {
        return new self($ordinal, Door::Time, NumFormat::invariant());
    }

    /**
     * A duration column, as HyperCast's {@see \HyperCast\Duration} pair.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @return self the column
     */
    public static function duration(int $ordinal): self
    {
        return new self($ordinal, Door::Duration, NumFormat::invariant());
    }

    /**
     * A text column: the cell's bytes themselves, untrimmed, quotes resolved. A cell with
     * no bytes at all is the one way text fails — an Empty fault.
     *
     * @param int $ordinal zero-based ordinal of the source column
     * @return self the column
     */
    public static function text(int $ordinal): self
    {
        return new self($ordinal, Door::Text, NumFormat::invariant());
    }
}
