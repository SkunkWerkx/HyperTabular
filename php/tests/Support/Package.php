<?php

declare(strict_types=1);

namespace HyperTabular\Tests\Support;

/**
 * A workbook package made here, for what no corpus package shows: a sheet of a given shape.
 */
final class Package
{
    private const MAIN = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"';

    /**
     * A ZIP of `$parts` (name to bytes), each deflated.
     *
     * @param array<string, string> $parts
     */
    public static function zip(array $parts): string
    {
        $body = '';
        $central = '';
        foreach ($parts as $name => $data) {
            $packed = gzdeflate($data);
            $shape = pack('v5V3v2', 20, 0, 8, 0, 0, crc32($data), \strlen($packed), \strlen($data), \strlen($name), 0);
            $central .= "PK\x01\x02" . pack('v', 20) . $shape . pack('v3V2', 0, 0, 0, 0, \strlen($body)) . $name;
            $body .= "PK\x03\x04" . $shape . $name . $packed;
        }
        return $body . $central
            . "PK\x05\x06" . pack('v4V2v', 0, 0, \count($parts), \count($parts), \strlen($central), \strlen($body), 0);
    }

    /** The smallest package a reader takes, with `$rows` as its one sheet's. */
    public static function xlsx(string $rows): string
    {
        return self::zip([
            '[Content_Types].xml' => '<Types/>',
            '_rels/.rels' => '<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/'
                . 'officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>',
            'xl/workbook.xml' => '<workbook ' . self::MAIN . ' xmlns:r="r"><sheets>'
                . '<sheet name="Data" sheetId="1" r:id="rId0"/></sheets></workbook>',
            'xl/_rels/workbook.xml.rels' => '<Relationships><Relationship Id="rId0" Type="http://x/worksheet" '
                . 'Target="worksheets/sheet0.xml"/></Relationships>',
            'xl/worksheets/sheet0.xml' => '<worksheet ' . self::MAIN . "><sheetData>{$rows}</sheetData></worksheet>",
        ]);
    }
}
