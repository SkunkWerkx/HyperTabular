<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * How a sheet of a {@see Workbook} is read.
 */
final class SheetOptions
{
    /**
     * States the options; each defaults to what most sheets want.
     *
     * @param bool $hasHeader whether the sheet's first row is a header, exposed through
     *     {@see Sheet::header()} and never delivered as a row
     * @param bool $skipEmptyRows whether a row with no cells is skipped rather than delivered
     *     as a row of empty cells
     * @param int $batchRows rows per batch, at least one
     * @throws \InvalidArgumentException when a batch could hold no rows
     */
    public function __construct(
        public readonly bool $hasHeader = true,
        public readonly bool $skipEmptyRows = true,
        public readonly int $batchRows = DelimitedReader::DEFAULT_BATCH_ROWS,
    ) {
        if ($batchRows < 1) {
            throw new \InvalidArgumentException("A batch must hold at least one row; got {$batchRows}");
        }
    }
}
