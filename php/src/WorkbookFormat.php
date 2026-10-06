<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * Which kind of workbook a {@see Workbook} is. The values are the core's codes.
 */
enum WorkbookFormat: int
{
    /** Office Open XML: .xlsx, .xlsm. */
    case Xlsx = 1;

    /** OpenDocument: .ods. */
    case Ods = 2;
}
