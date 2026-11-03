<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Row;
use HyperTabular\RowIterator;
use HyperTabular\SheetOptions;
use HyperTabular\Tabular;
use HyperTabular\Tests\Support\ChildProcess;
use HyperTabular\Workbook;
use PHPUnit\Framework\TestCase;

/**
 * The extension as PHP sees it: its name and version, that it loads with no other extension,
 * and that HyperCast's, which judges every cell, is looked up when a cell needs it.
 */
final class TabularTest extends TestCase
{
    use ChildProcess;

    public function testTheExtensionReportsTheCratesVersion(): void
    {
        self::assertTrue(\extension_loaded('hypertabular'));
        self::assertSame(self::crateVersion(), phpversion('hypertabular'));
        self::assertSame(self::crateVersion(), Tabular::nativeVersion());
    }

    public function testIsAvailableIsTheNonThrowingProbe(): void
    {
        self::assertTrue(Tabular::isAvailable());
        self::assertTrue(Tabular::isAvailable());
    }

    public function testItLoadsWithNoOtherExtensionAndSaysWhatIsMissing(): void
    {
        // -n drops every ini file, and with them HyperCast's extension: this one loads, answers
        // its probe, and a read that needs a cell judged says which extension to install.
        $out = self::child(
            ['-n', '-d', 'extension=' . self::ext()],
            'var_export(HyperTabular\Tabular::isAvailable()); echo " ";'
            . 'echo HyperTabular\Tabular::nativeVersion(), " ";'
            . '$reader = HyperTabular\DelimitedReader::fromString("a\n1\n", HyperTabular\Dialect::csv());'
            . 'echo implode(",", $reader->header()), " ";'
            . 'try { HyperTabular\Column::i64(0); }'
            . 'catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(); }'
        );
        self::assertSame(
            'false ' . self::crateVersion() . ' a RuntimeException: hypertabular: the hypercast extension is not '
            . 'loaded; HyperTabular judges every cell with HyperCast (pie install skunkwerkx/hypercast)',
            $out
        );
    }

    public function testTheExtensionsLoadInEitherOrder(): void
    {
        $code = 'echo HyperTabular\DelimitedReader::fromString("a\n7\n", HyperTabular\Dialect::csv(),'
            . ' [HyperTabular\Column::i64(0)])->read()->get(0, 0)->value;';
        self::assertSame('7', self::child(['-n', '-d', self::hypercast(), '-d', 'extension=' . self::ext()], $code));
        self::assertSame('7', self::child(['-n', '-d', 'extension=' . self::ext(), '-d', self::hypercast()], $code));
    }

    public function testTheReadersAreNotConstructedOrSerialized(): void
    {
        $reader = DelimitedReader::fromString("a\n1\n", Dialect::csv(), [Column::i64(0)]);
        foreach ([$reader, $reader->read()] as $object) {
            try {
                serialize($object);
                self::fail(get_class($object) . ' serialized');
            } catch (\Exception $e) {
                self::assertStringContainsString('is not allowed', $e->getMessage());
            }
        }
        $this->expectException(\Error::class);
        $this->expectExceptionMessage('Call to private HyperTabular\\Batch::__construct()');
        new Batch();
    }

    public function testAReadersRowsAreForwardOnlyAsAGeneratorsWere(): void
    {
        $reader = DelimitedReader::fromString("a\n1\n2\n3\n", Dialect::csv(), [Column::i64(0)], batchRows: 2);
        $rows = $reader->rows();
        self::assertInstanceOf(RowIterator::class, $rows);
        $seen = [];
        foreach ($rows as $key => $row) {
            $seen[$key] = $row->get(0)->value;
        }
        self::assertSame([1, 2, 3], $seen);
        $this->expectException(\Exception::class);
        $this->expectExceptionMessage('Cannot rewind a generator that was already run');
        foreach ($rows as $row) {
            self::fail('rewound');
        }
    }

    public function testABatchsRowsRewind(): void
    {
        $batch = DelimitedReader::fromString("a\n1\n2\n", Dialect::csv(), [Column::i64(0)])->read();
        $first = array_map(static fn (Row $row): int => $row->get(0)->value, iterator_to_array($batch));
        $second = array_map(static fn (Row $row): int => $row->get(0)->value, iterator_to_array($batch));
        self::assertSame([1, 2], $first);
        self::assertSame($first, $second);
    }

    public function testASheetIsChosenByIntOrString(): void
    {
        $book = Workbook::open(\dirname(__DIR__, 2) . '/corpus/workbook/basic.xlsx');
        $name = $book->sheets()[0]->name;
        self::assertSame(
            $book->sheet(0, new SheetOptions())->header(),
            $book->sheet($name, new SheetOptions())->header()
        );
        $this->expectException(\TypeError::class);
        $this->expectExceptionMessage(
            'HyperTabular\\Workbook::sheet(): Argument #1 ($which) must be of type string|int, array given'
        );
        $book->sheet([], new SheetOptions());
    }

    /** The crate's own manifest version. */
    private static function crateVersion(): string
    {
        $manifest = (string) file_get_contents(\dirname(__DIR__, 2) . '/rust/Cargo.toml');
        self::assertSame(1, preg_match('/^version\s*=\s*"([^"]+)"/m', $manifest, $match));
        return $match[1];
    }
}
