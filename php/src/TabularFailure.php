<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * Why an input could not be read as rows at all — the kinds of {@see TabularException}. The
 * values are the core's codes.
 */
enum TabularFailure: int
{
    /** The input ended inside a quoted cell. */
    case UnclosedQuote = 1;

    /** A record's cell count disagrees with the first record's. */
    case ColumnCount = 2;

    /** A single record is larger than {@see DelimitedReader::MAX_ROW_BYTES}. */
    case RowTooLong = 3;

    /** The workbook's container is not a zip file. */
    case NotAZip = 16;

    /** The zip's own structure is broken. */
    case Container = 17;

    /** The workbook is encrypted. */
    case Encrypted = 18;

    /** A part is compressed by a method other than stored or deflate. */
    case Method = 19;

    /** A part the workbook cannot be read without is missing. */
    case MissingPart = 20;

    /** A part's XML ends inside a construct. */
    case Xml = 21;

    /** A part's bytes are not a deflate stream, or stop before the stream does. */
    case Deflate = 22;

    /** The zip is neither an XLSX nor an ODS workbook. */
    case NotAWorkbook = 23;

    /** A cell names a shared string the table does not have. */
    case SharedString = 24;

    /** More text than can be addressed: over 4 GiB in a batch or in the shared strings, or 2 GiB in a cell. */
    case TooLarge = 25;
}
