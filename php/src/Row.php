<?php

declare(strict_types=1);

namespace HyperTabular;

use HyperCast\Fault;
use HyperCast\Success;

/**
 * One row of a {@see Batch}: the batch read across rather than down, for code that builds a
 * value a row at a time.
 *
 * ```php
 * foreach ($reader->rows() as $row) {
 *     $id = $row->get(0);                          // HyperCast's union, as Batch::get()
 *     $name = $row->value(1);                      // the value, or null, as Batch::values()
 *     if ($id instanceof Fault) {
 *         echo "line {$row->line()}: {$id->reason->name} in {$row->raw(0)}\n";
 *     }
 * }
 * ```
 *
 * Each method is the batch's own with this row's index, so a row answers exactly what its
 * batch does. A batch owns what it shows, and so does a row: it stays good after the reader
 * has moved on.
 */
final class Row
{
    /**
     * Made by {@see Batch::getIterator()}.
     *
     * @param Batch $batch the batch the row is in
     * @param int $index the row's place in it, from zero
     * @internal
     */
    public function __construct(
        private readonly Batch $batch,
        private readonly int $index,
    ) {
    }

    /**
     * The batch the row is in.
     *
     * @return Batch the batch
     */
    public function batch(): Batch
    {
        return $this->batch;
    }

    /**
     * The row's place in its batch, from zero.
     *
     * @return int the index
     */
    public function index(): int
    {
        return $this->index;
    }

    /**
     * Where the row came from: {@see Batch::line()}.
     *
     * @return int the line or row number
     */
    public function line(): int
    {
        return $this->batch->line($this->index);
    }

    /**
     * The cell in `$column` as HyperCast judged it: {@see Batch::get()}.
     *
     * @param int $column the plan column
     * @return Success|Fault the verdict
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function get(int $column): Success|Fault
    {
        return $this->batch->get($column, $this->index);
    }

    /**
     * The value of the cell in `$column`, or null where it did not cast: the row's entry of
     * {@see Batch::values()}.
     *
     * @param int $column the plan column
     * @return mixed the PHP carrier of the column's door ({@see Door}), or null
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function value(int $column): mixed
    {
        return $this->batch->values($column)[$this->index];
    }

    /**
     * Why the cell in `$column` did not cast, or null when it did: the row's entry of
     * {@see Batch::faults()}.
     *
     * @param int $column the plan column
     * @return Fault|null the fault
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function fault(int $column): ?Fault
    {
        return $this->batch->faults($column)[$this->index] ?? null;
    }

    /**
     * The text the cell in `$column` was cast from: {@see Batch::raw()}.
     *
     * @param int $column the plan column
     * @return string the cell's bytes
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function raw(int $column): string
    {
        return $this->batch->raw($column, $this->index);
    }
}
