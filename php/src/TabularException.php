<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * A structural failure: the input is not rows of cells — a record of the wrong width,
 * input that ends inside a quoted cell. Never a cell's verdict: a value that does not cast
 * is a Fault in its column, and the read goes on. A structural failure ends the input,
 * after every intact row before it has been delivered.
 *
 * The position is the input's, not PHP's: {@see $recordLine} is the line of the *data*
 * the record starts on (an Exception's own `getLine()` is the PHP source line it was
 * thrown from, which is why the property is not simply called `line`).
 */
final class TabularException extends \RuntimeException
{
    /**
     * Carries the failure as the core reported it.
     *
     * @param TabularFailure $kind what is wrong
     * @param int $record zero-based index of the offending record — the header and skipped
     *     blank lines included
     * @param int $recordLine one-based line of the input the offending record starts on
     * @param int $byte absolute byte offset of the offending record's start
     * @param int $expected cells in the first record, for {@see TabularFailure::ColumnCount}
     * @param int $found cells in this record, for {@see TabularFailure::ColumnCount}
     */
    public function __construct(
        public readonly TabularFailure $kind,
        public readonly int $record,
        public readonly int $recordLine,
        public readonly int $byte,
        public readonly int $expected = 0,
        public readonly int $found = 0,
    ) {
        parent::__construct(match ($kind) {
            TabularFailure::ColumnCount =>
                "Record {$record} (line {$recordLine}, byte {$byte}) has {$found} cells; "
                . "the first record had {$expected}.",
            TabularFailure::UnclosedQuote =>
                "The input ended inside a quoted cell in record {$record} (line {$recordLine}, byte {$byte}).",
            TabularFailure::RowTooLong =>
                "Record {$record} (line {$recordLine}, byte {$byte}) exceeds the "
                . DelimitedReader::MAX_ROW_BYTES . '-byte row ceiling.',
        });
    }
}
