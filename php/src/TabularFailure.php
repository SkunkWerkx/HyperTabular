<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * Why an input could not be read as rows at all — the kinds of {@see TabularException}.
 */
enum TabularFailure: int
{
    /** The input ended inside a quoted cell. */
    case UnclosedQuote = 1;

    /** A record's cell count disagrees with the first record's. */
    case ColumnCount = 2;

    /** A single record is larger than {@see DelimitedReader::MAX_ROW_BYTES}. */
    case RowTooLong = 3;
}
