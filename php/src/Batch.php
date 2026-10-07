<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;
use HyperCast\Decimal;
use HyperCast\Duration;
use HyperCast\Fault;
use HyperCast\Interop\NativeValues;
use HyperCast\Success;

/**
 * The rows one {@see DelimitedReader::read()} or {@see Sheet::read()} delivered, as typed
 * columns: the same class for delimited text and for a sheet of a workbook.
 *
 * ```php
 * while (($batch = $reader->read()) !== null) {
 *     $ids = $batch->values(0);                    // a column at a time
 *     for ($row = 0; $row < $batch->rows(); $row++) {
 *         $verdict = $batch->get(2, $row);         // or a cell at a time, as HyperCast's union
 *         if ($verdict instanceof Fault) {
 *             echo "line {$batch->line($row)}: {$verdict->reason->name} in {$batch->raw(2, $row)}\n";
 *         }
 *     }
 * }
 * ```
 *
 * A batch owns what it shows — the arrays the core wrote are copied out of the reader's
 * buffers as it is made — so it stays good after the reader has moved on. A value is the
 * PHP carrier HyperCast's `Cast` returns for the column's door ({@see Door} lists them).
 */
final class Batch
{
    /** @var array<int, list<mixed>> each decoded column's values, null where the cell did not cast */
    private array $values = [];
    /** @var array<int, array<int, Fault>> each decoded column's faults, by row */
    private array $faults = [];
    private ?FFI\CData $scratch = null;
    private int $scratchCap = 0;

    /**
     * Made by a reader from what the core just wrote, copied.
     *
     * @param list<Column> $plan the output columns
     * @param int $rows rows in the batch
     * @param list<string> $valueBytes each column's value array
     * @param list<string> $verdictBytes each column's verdict array
     * @param string $cells the cell table, `$perRow` spans a row
     * @param int $perRow cell-table entries one row takes
     * @param string $base what an unflagged span indexes: the input the rows were read from,
     *     or a workbook's shared strings
     * @param string $arena what a flagged span indexes
     * @param bool $workbook whether the rows are a sheet's
     * @internal
     */
    public function __construct(
        private readonly array $plan,
        private readonly int $rows,
        private readonly array $valueBytes,
        private readonly array $verdictBytes,
        private readonly string $cells,
        private readonly int $perRow,
        private readonly string $base,
        private readonly string $arena,
        private readonly bool $workbook,
    ) {
    }

    /**
     * How many rows the batch holds — never zero.
     *
     * @return int the number of rows every column of the batch holds
     */
    public function rows(): int
    {
        return $this->rows;
    }

    /**
     * The plan the batch was read through: column `i` of the batch is `columns()[i]`.
     *
     * @return list<Column> the output columns, in output order
     */
    public function columns(): array
    {
        return $this->plan;
    }

    /**
     * Where a row came from: for delimited text the 1-based line its record starts on, for
     * a sheet its 1-based row number.
     *
     * @param int $row the row within the batch
     * @return int the line or row number
     * @throws \OutOfRangeException when the batch has no such row
     */
    public function line(int $row): int
    {
        $this->checkRow($row);
        $entry = unpack('Voffset/Vlen', $this->cells, ($row * $this->perRow + $this->perRow - 1) * 8);
        return $this->workbook ? $entry['offset'] : $entry['len'];
    }

    /**
     * A column, one value per row, each the PHP carrier of the column's door ({@see Door})
     * — and null where the cell did not cast, whatever the reason; {@see faults()} says why.
     * Built in one piece the first time it is asked for.
     *
     * @param int $column the plan column
     * @return list<mixed> the values, by row
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function values(int $column): array
    {
        $this->decode($column);
        return $this->values[$column];
    }

    /**
     * The cells of a column that did not cast: HyperCast's `Fault` for each, keyed by row.
     * Empty when every cell cast.
     *
     * @param int $column the plan column
     * @return array<int, Fault> the faults, by row, in row order
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function faults(int $column): array
    {
        $this->decode($column);
        return $this->faults[$column];
    }

    /**
     * A column as verdicts, one per row: HyperCast's `Success` around the value, or its
     * `Fault`.
     *
     * @param int $column the plan column
     * @return list<Success|Fault> the verdicts, by row
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function verdicts(int $column): array
    {
        $this->decode($column);
        $faults = $this->faults[$column];
        $verdicts = [];
        foreach ($this->values[$column] as $row => $value) {
            $verdicts[] = $faults[$row] ?? new Success($value);
        }
        return $verdicts;
    }

    /**
     * The cell at (`$column`, `$row`) as HyperCast judged it: its `Success` around the value
     * — the PHP carrier of the column's door ({@see Door}) — or its `Fault`, whose span
     * indexes the cell's own text ({@see raw()}).
     *
     * @param int $column the plan column
     * @param int $row the row within the batch
     * @return Success|Fault the verdict
     * @throws \OutOfRangeException when the plan has no such column or the batch no such row
     */
    public function get(int $column, int $row): Success|Fault
    {
        $this->decode($column);
        $this->checkRow($row);
        return $this->faults[$column][$row] ?? new Success($this->values[$column][$row]);
    }

    /**
     * The text a cell was cast from, whatever its door and whatever its verdict — what a
     * fault's span indexes, and what to show for a value that did not cast. Quotes are
     * resolved, as the core resolved them before casting. For a sheet this is a text cell's
     * own text, and what a typed cell was said as when it failed its door (or went through
     * the text door); a typed cell that cast has none.
     *
     * @param int $column the plan column
     * @param int $row the row within the batch
     * @return string the cell's bytes
     * @throws \OutOfRangeException when the plan has no such column or the batch no such row
     */
    public function raw(int $column, int $row): string
    {
        $this->checkColumn($column);
        $this->checkRow($row);
        $at = $row * $this->perRow + ($this->workbook ? $column : $this->plan[$column]->ordinal);
        $cell = unpack('Voffset/Vlen', $this->cells, $at * 8);
        if ($this->workbook) {
            return $this->slice($cell['offset'], $cell['len']);
        }
        $length = $cell['len'] & Native::SPAN_LENGTH;
        if ($length === 0) {
            return '';
        }
        $raw = substr($this->base, $cell['offset'], $length);
        if (($cell['len'] & Native::SPAN_FLAG) === 0) {
            return $raw;
        }
        // A cell with an escaped quote in it: unescaped by the core, as the core cast it.
        $ffi = Native::ffi();
        if ($this->scratchCap < $length) {
            $this->scratchCap = max($length, 256);
            $this->scratch = $ffi->new("uint8_t[{$this->scratchCap}]");
        }
        $written = $ffi->hypertabular_delimited_unescape($raw, $length, $this->scratch, $this->scratchCap);
        return FFI::string($this->scratch, $written);
    }

    /**
     * The bytes a span names: in the arena when it is flagged, in the base when not.
     *
     * @param int $offset the span's offset
     * @param int $length the span's length, its flag included
     * @return string the bytes
     */
    private function slice(int $offset, int $length): string
    {
        return ($length & Native::SPAN_FLAG) !== 0
            ? substr($this->arena, $offset, $length & Native::SPAN_LENGTH)
            : substr($this->base, $offset, $length);
    }

    /**
     * Refuses a row the batch does not have.
     *
     * @param int $row the row within the batch
     * @return void
     */
    private function checkRow(int $row): void
    {
        if ($row < 0 || $row >= $this->rows) {
            throw new \OutOfRangeException("The batch has {$this->rows} rows; there is no row {$row}");
        }
    }

    /**
     * Refuses a column the plan does not have.
     *
     * @param int $column the plan column
     * @return void
     */
    private function checkColumn(int $column): void
    {
        $count = \count($this->plan);
        if ($column < 0 || $column >= $count) {
            throw new \OutOfRangeException("The plan has {$count} columns; there is no column {$column}");
        }
    }

    /**
     * Lifts one column out of its arrays — the verdicts and the values, each unpacked in one
     * piece — unless it already has been.
     *
     * @param int $column the plan column
     * @return void
     */
    private function decode(int $column): void
    {
        if (isset($this->values[$column])) {
            return;
        }
        $this->checkColumn($column);
        $rows = $this->rows;

        // A verdict is three little-endian u32s — offset, length, reason — and reason 0 is
        // a cell that cast, with nothing else set: a column that all cast is all zero bytes,
        // which strspn() answers without unpacking it. unpack() numbers from 1.
        $faults = [];
        $verdictBytes = $this->verdictBytes[$column];
        if (strspn($verdictBytes, "\0") !== \strlen($verdictBytes)) {
            $verdicts = unpack('V*', $verdictBytes);
            for ($row = 0, $at = 3; $row < $rows; $row++, $at += 3) {
                if ($verdicts[$at] !== 0) {
                    $faults[$row] = NativeValues::fault($verdicts[$at], $verdicts[$at - 2], $verdicts[$at - 1]);
                }
            }
        }

        $door = $this->plan[$column]->door;
        $bytes = $this->valueBytes[$column];
        $values = match ($door) {
            Door::Bool => self::booleans($bytes),
            Door::I8 => array_values(unpack('c*', $bytes)),
            Door::I16 => array_values(unpack('s*', $bytes)),
            Door::I32 => array_values(unpack('l*', $bytes)),
            // u64 rides PHP's signed int as its two's-complement bit pattern, as HyperCast's
            // own u64 door presents it; a time of day is far inside the signed range.
            Door::I64, Door::U64, Door::Time => array_values(unpack('q*', $bytes)),
            Door::U8 => array_values(unpack('C*', $bytes)),
            Door::U16 => array_values(unpack('v*', $bytes)),
            Door::U32 => array_values(unpack('V*', $bytes)),
            Door::F32 => array_values(unpack('g*', $bytes)),
            Door::F64 => array_values(unpack('e*', $bytes)),
            default => $this->structured($door, $bytes, $rows, $faults),
        };
        foreach ($faults as $row => $_) {
            $values[$row] = null;
        }
        $this->values[$column] = $values;
        $this->faults[$column] = $faults;
    }

    /**
     * A bool column's values: one byte a cell, 0 or 1.
     *
     * @param string $bytes the column's value array
     * @return list<bool> the values, by row
     */
    private static function booleans(string $bytes): array
    {
        $values = [];
        foreach (unpack('C*', $bytes) as $byte) {
            $values[] = $byte !== 0;
        }
        return $values;
    }

    /**
     * The values of a column whose door writes more than one number per cell, built into
     * the carriers HyperCast's `Cast` returns for the same doors. A cell that did not cast
     * is null here already, and is never built.
     *
     * Each column is unpacked whole, in 32- or 64-bit words, and a cell's fields taken from
     * its words: one unpack() a column rather than one a cell, which is most of what a cell
     * would otherwise cost. The layouts are `kernel::abi`'s, little-endian, and the padding
     * in them is masked off rather than trusted to be zero.
     *
     * @param Door $door the column's door
     * @param string $bytes the column's value array
     * @param int $rows rows in the batch
     * @param array<int, Fault> $faults the column's faults, by row
     * @return list<mixed> the values, by row
     */
    private function structured(Door $door, string $bytes, int $rows, array $faults): array
    {
        $values = [];
        switch ($door) {
            case Door::Text:
                // Offset, then length: a span into the input the batch was read from or the
                // shared strings — or, flagged, into the arena.
                $spans = unpack('V*', $bytes);
                for ($row = 0, $at = 1; $row < $rows; $row++, $at += 2) {
                    if (isset($faults[$row])) {
                        $values[] = null;
                        continue;
                    }
                    $length = $spans[$at + 1];
                    $values[] = ($length & Native::SPAN_FLAG) !== 0
                        ? substr($this->arena, $spans[$at], $length & Native::SPAN_LENGTH)
                        : substr($this->base, $spans[$at], $length);
                }
                return $values;
            case Door::Date:
            case Door::DateOrdered:
                // Year (u16), month, day: one 32-bit word.
                foreach (unpack('V*', $bytes) as $index => $word) {
                    $values[] = isset($faults[$index - 1])
                        ? null
                        : NativeValues::date($word & 0xFFFF, ($word >> 16) & 0xFF, $word >> 24);
                }
                return $values;
            case Door::Timestamp:
            case Door::Unix:
            case Door::ExcelSerial:
                // Seconds (i64), then nanoseconds (i32, never negative) and four bytes of
                // padding.
                $words = unpack('q*', $bytes);
                for ($row = 0, $at = 1; $row < $rows; $row++, $at += 2) {
                    $values[] = isset($faults[$row])
                        ? null
                        : NativeValues::instant($words[$at], $words[$at + 1] & 0xFFFFFFFF);
                }
                return $values;
            case Door::DateTime:
                // The date's word and four bytes of padding, then nanoseconds of the day
                // (u64).
                $words = unpack('q*', $bytes);
                for ($row = 0, $at = 1; $row < $rows; $row++, $at += 2) {
                    if (isset($faults[$row])) {
                        $values[] = null;
                        continue;
                    }
                    $date = $words[$at];
                    $values[] = NativeValues::civil(
                        $date & 0xFFFF,
                        ($date >> 16) & 0xFF,
                        ($date >> 24) & 0xFF,
                        $words[$at + 1]
                    );
                }
                return $values;
            case Door::Duration:
                // Seconds (i64), then same-signed nanoseconds (i32) and four bytes of padding.
                $words = unpack('q*', $bytes);
                for ($row = 0, $at = 1; $row < $rows; $row++, $at += 2) {
                    if (isset($faults[$row])) {
                        $values[] = null;
                        continue;
                    }
                    $nanos = $words[$at + 1] & 0xFFFFFFFF;
                    $values[] = new Duration($words[$at], $nanos >= 0x80000000 ? $nanos - 0x100000000 : $nanos);
                }
                return $values;
            case Door::Decimal:
                // The low 64 bits of the magnitude, then its high 32 bits, the scale, the sign
                // and two bytes of padding.
                $words = unpack('q*', $bytes);
                for ($row = 0, $at = 1; $row < $rows; $row++, $at += 2) {
                    if (isset($faults[$row])) {
                        $values[] = null;
                        continue;
                    }
                    $high = $words[$at + 1];
                    $values[] = Decimal::fromLimbs(
                        $words[$at],
                        $high & 0xFFFFFFFF,
                        ($high >> 32) & 0xFF,
                        (($high >> 40) & 0xFF) !== 0
                    );
                }
                return $values;
            case Door::Uuid:
                for ($row = 0; $row < $rows; $row++) {
                    $values[] = isset($faults[$row]) ? null : NativeValues::uuid(substr($bytes, $row * 16, 16));
                }
                return $values;
            default:
                throw new \LogicException("No carrier for door {$door->name}");
        }
    }
}
