<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * The door a column is cast through: HyperCast's, plus {@see Door::Text} for the bytes
 * themselves. Named as HyperCast's PHP binding names its doors, and numbered as the core
 * numbers them. Each case says what a cell of that door is in PHP — the same carrier
 * HyperCast's own `Cast` returns for it.
 */
enum Door: int
{
    /** `bool`, HyperCast's boolean lexicon. */
    case Bool = 1;
    /** A signed 8-bit integer, as `int`. */
    case I8 = 2;
    /** A signed 16-bit integer, as `int`. */
    case I16 = 3;
    /** A signed 32-bit integer, as `int`. */
    case I32 = 4;
    /** A signed 64-bit integer, as `int`. */
    case I64 = 5;
    /** An unsigned 8-bit integer, as `int`. */
    case U8 = 6;
    /** An unsigned 16-bit integer, as `int`. */
    case U16 = 7;
    /** An unsigned 32-bit integer, as `int`. */
    case U32 = 8;
    /** An unsigned 64-bit integer, as `int`'s two's-complement bit pattern — render with sprintf('%u'). */
    case U64 = 9;
    /** An IEEE single, as `float` (widened losslessly). */
    case F32 = 10;
    /** An IEEE double, as `float`. */
    case F64 = 11;
    /** A UUID, as the lowercase hyphenated string. */
    case Uuid = 12;
    /** An RFC 3339 instant, as a UTC DateTimeImmutable (nanoseconds truncate to microseconds). */
    case Timestamp = 13;
    /** A Unix-epoch integer at a declared precision, as a UTC DateTimeImmutable. */
    case Unix = 14;
    /** A strict `yyyy-MM-dd` date, as a DateTimeImmutable at UTC midnight. */
    case Date = 15;
    /** A 24-hour time of day, as an exact `int` of nanoseconds since midnight. */
    case Time = 16;
    /** A duration, as HyperCast's {@see \HyperCast\Duration} pair. */
    case Duration = 17;
    /** The cell's bytes themselves, as `string` — no cast. */
    case Text = 18;
    /** An exact decimal, as HyperCast's {@see \HyperCast\Decimal} triple; no float is ever formed. */
    case Decimal = 19;
    /** A separated calendar date under a declared field order, as a DateTimeImmutable at UTC midnight. */
    case DateOrdered = 20;
    /** A zone-less civil date-time under a declared field order, as a UTC-labeled DateTimeImmutable. */
    case DateTime = 21;
    /** An Excel date serial under a declared date system, as a UTC DateTimeImmutable. */
    case ExcelSerial = 22;

    /**
     * Bytes one value of this door takes in a column buffer — the core's layout
     * (rust/src/kernel/abi.rs, `ColumnBuffer`).
     *
     * @internal the reader's allocation arithmetic
     * @return int the value's size in bytes
     */
    public function valueSize(): int
    {
        return match ($this) {
            self::Bool, self::I8, self::U8 => 1,
            self::I16, self::U16 => 2,
            self::I32, self::U32, self::F32, self::Date, self::DateOrdered => 4,
            self::I64, self::U64, self::F64, self::Time, self::Text => 8,
            self::Decimal, self::Uuid, self::Timestamp, self::Unix, self::ExcelSerial,
            self::DateTime, self::Duration => 16,
        };
    }
}
