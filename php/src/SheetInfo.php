<?php

declare(strict_types=1);

namespace HyperTabular;

/**
 * One sheet of a workbook, as {@see Workbook::sheets()} lists it.
 */
final class SheetInfo
{
    /**
     * Made by {@see Workbook} as it lists the sheets.
     *
     * @param string $name the sheet's name
     * @param bool $hidden whether the workbook hides the sheet — a hidden sheet reads like any other
     * @param string $part the XLSX part that holds it, as the core is given it back
     * @param int $index the ODS table's index, likewise
     * @internal
     */
    public function __construct(
        public readonly string $name,
        public readonly bool $hidden,
        public readonly string $part,
        public readonly int $index,
    ) {
    }
}
