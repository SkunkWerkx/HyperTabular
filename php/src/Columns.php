<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;
use FFI\CData;

/**
 * A plan as the core takes it, and the arrays the core casts it into: the column specs,
 * one value array and one verdict array per column sized for one batch, and the column
 * buffer table that names them — FFI memory allocated once and reused for every batch. A
 * {@see DelimitedReader} and a {@see Sheet} each own one.
 *
 * @internal FFI plumbing, not part of the public API — PHP has no package-private visibility.
 */
final class Columns
{
    /** @var list<Column> */
    public readonly array $plan;
    public readonly int $count;
    public readonly int $batchRows;
    /** The widest ordinal the plan reads, plus one. */
    public readonly int $width;
    public readonly CData $specs;
    public readonly CData $buffers;
    /** @var list<CData> */
    private array $values = [];
    /** @var list<CData> */
    private array $verdicts = [];

    /**
     * Checks the plan and allocates its arrays.
     *
     * @param array<mixed> $plan the output columns, in output order
     * @param int $batchRows rows per batch
     * @throws \InvalidArgumentException when the batch size or the plan cannot be honoured
     */
    public function __construct(array $plan, int $batchRows)
    {
        if ($batchRows < 1) {
            throw new \InvalidArgumentException("A batch must hold at least one row; got {$batchRows}");
        }
        $plan = array_values($plan);
        foreach ($plan as $index => $column) {
            if (!$column instanceof Column) {
                throw new \InvalidArgumentException(
                    "Plan entry {$index} is not a Column; build one with Column's factories"
                );
            }
        }
        $ffi = Native::ffi();
        $this->plan = $plan;
        $this->count = \count($plan);
        $this->batchRows = $batchRows;

        // FFI cannot allocate nothing, and a plan may be empty (rows are still counted).
        $slots = max(1, $this->count);
        $this->specs = $ffi->new("ht_column_spec[{$slots}]");
        $this->buffers = $ffi->new("ht_column_buffer[{$slots}]");
        $widest = -1;
        foreach ($plan as $index => $column) {
            $widest = max($widest, $column->ordinal);
            $spec = $this->specs[$index];
            $spec->ordinal = $column->ordinal;
            $spec->door = $column->door->value;
            $spec->param = $column->declared;
            // HyperCast's NumFormat in the core's 32-byte layout. The object validated itself
            // when it was built, as HyperCast validates it, so the core has nothing to refuse.
            [$decimal, $group] = $column->format->codePoints();
            $spec->format->decimal_sep = $decimal;
            $spec->format->group_sep = $group;
            $spec->format->flags = $column->format->flags;
            $currency = $column->format->currency;
            $spec->format->currency_len = \strlen($currency);
            if ($currency !== '') {
                FFI::memcpy($spec->format->currency, $currency, \strlen($currency));
            }

            $values = $ffi->new('uint8_t[' . $batchRows * $column->door->valueSize() . ']');
            $verdicts = $ffi->new("ht_verdict[{$batchRows}]");
            $this->values[] = $values;
            $this->verdicts[] = $verdicts;
            $this->buffers[$index]->values = FFI::addr($values);
            $this->buffers[$index]->verdicts = $ffi->cast('ht_verdict *', FFI::addr($verdicts));
        }
        $this->width = $widest + 1;
    }

    /**
     * The first `$rows` values of every column, as the core wrote them, copied out.
     *
     * @param int $rows rows in the batch
     * @return list<string> one string of values per column
     */
    public function values(int $rows): array
    {
        $copies = [];
        foreach ($this->plan as $index => $column) {
            $copies[] = FFI::string($this->values[$index], $rows * $column->door->valueSize());
        }
        return $copies;
    }

    /**
     * The first `$rows` verdicts of every column, as the core wrote them, copied out.
     *
     * @param int $rows rows in the batch
     * @return list<string> one string of verdicts per column
     */
    public function verdicts(int $rows): array
    {
        $copies = [];
        foreach ($this->verdicts as $verdicts) {
            $copies[] = FFI::string($verdicts, $rows * 12);
        }
        return $copies;
    }
}
