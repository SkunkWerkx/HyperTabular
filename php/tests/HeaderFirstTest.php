<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperCast\CastFailure;
use HyperCast\Fault;
use HyperCast\Success;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Header;
use HyperTabular\Row;
use HyperTabular\SheetOptions;
use HyperTabular\TabularException;
use HyperTabular\TabularFailure;
use HyperTabular\Workbook;
use PHPUnit\Framework\TestCase;

/**
 * Opening a source before its plan is known — the header read, the names looked up, the plan
 * bound — and reading it a row at a time. The corpus replays both through every source;
 * these are the surface and the caller bugs it does not reach.
 */
final class HeaderFirstTest extends TestCase
{
    private const ORDERS = "id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n3,,7\n";

    private static function workbook(string $name): string
    {
        return \dirname(__DIR__, 2) . "/corpus/workbook/{$name}";
    }

    public function testThePlanIsBuiltFromTheHeadersNames(): void
    {
        $reader = DelimitedReader::fromString(self::ORDERS, Dialect::csv());
        $this->assertFalse($reader->isBound());
        $this->assertSame([], $reader->plan());
        $this->assertSame(['id', 'name', 'score'], $reader->header());
        $this->assertSame(3, $reader->columnCount());

        $header = $reader->headerIndex();
        $this->assertInstanceOf(Header::class, $header);
        $this->assertSame($header, $reader->headerIndex());
        $plan = [Column::f64($header->ordinal('score')), Column::i32($header->ordinal('id'))];
        $reader->bind($plan);
        $this->assertTrue($reader->isBound());
        $this->assertSame($plan, $reader->plan());

        $this->assertNotNull($batch = $reader->read());
        $this->assertSame([1, 2, 3], $batch->values(1));
        $this->assertSame([2.5, null, 7.0], $batch->values(0));
        $this->assertNull($reader->read());
    }

    public function testANameIsMatchedExactlyAndAtItsFirstColumn(): void
    {
        $reader = DelimitedReader::fromString("a,Name, name,a,7\n", Dialect::csv());
        $header = $reader->headerIndex();
        $this->assertSame(0, $header->ordinal('a'));
        $this->assertSame(1, $header->find('Name'));
        $this->assertSame(2, $header->find(' name'));
        $this->assertSame(4, $header->ordinal('7'));
        $this->assertNull($header->find('name'));
        $this->assertNull($header->find('NAME'));
        $this->assertNull($header->find('07'));
        try {
            $header->ordinal('name');
            $this->fail('a name the header does not have was found');
        } catch (\OutOfBoundsException $missing) {
            $this->assertStringContainsString('"name"', $missing->getMessage());
        }
    }

    public function testAHeaderStillReadsAsItsList(): void
    {
        $header = DelimitedReader::fromString(self::ORDERS, Dialect::csv())->headerIndex();
        $this->assertSame(['id', 'name', 'score'], $header->names());
        $this->assertCount(3, $header);
        $this->assertSame(['id', 'name', 'score'], iterator_to_array($header));
        $this->assertSame('name', $header[1]);
        $this->assertTrue(isset($header[2]));
        $this->assertFalse(isset($header[3]));
        $this->assertSame('["id","name","score"]', json_encode($header));
        foreach (
            [
                static fn () => $header[3],
                static fn () => $header['id'],
            ] as $ask
        ) {
            try {
                $ask();
                $this->fail('expected an OutOfRangeException');
            } catch (\OutOfRangeException) {
                $this->addToAssertionCount(1);
            }
        }
        try {
            $header[0] = 'key';
            $this->fail('a header was changed');
        } catch (\LogicException) {
            $this->assertSame('id', $header[0]);
        }
    }

    public function testNoHeaderDeclaredIsNoHeaderIndex(): void
    {
        $reader = DelimitedReader::fromString("1,x\n2,y\n", new Dialect(',', hasHeader: false));
        $this->assertNull($reader->header());
        $this->assertNull($reader->headerIndex());
        // Without a header the width is the first record's, known once it has been read.
        $this->assertNull($reader->columnCount());
        $reader->bind([Column::text(1)]);
        $this->assertNotNull($batch = $reader->read());
        $this->assertSame(['x', 'y'], $batch->values(0));
        $this->assertSame(2, $reader->columnCount());
    }

    public function testAReadBeforeAPlanIsACallerBugThatDoesNotStick(): void
    {
        $reader = DelimitedReader::fromString(self::ORDERS, Dialect::csv(), batchRows: 2);
        for ($tries = 0; $tries < 2; $tries++) {
            try {
                $reader->read();
                $this->fail('an unbound reader read');
            } catch (\LogicException $unbound) {
                $this->assertStringContainsString('bind()', $unbound->getMessage());
            }
        }
        $reader->bind([Column::i32(0)]);
        $this->assertSame([1, 2], $reader->read()->values(0));
        $this->assertSame([3], $reader->read()->values(0));
        $this->assertNull($reader->read());
    }

    public function testAPlanIsBoundOnce(): void
    {
        $bound = DelimitedReader::fromString(self::ORDERS, Dialect::csv(), [Column::i32(0)]);
        $unbound = DelimitedReader::fromString(self::ORDERS, Dialect::csv());
        $unbound->bind([Column::i32(0)]);
        foreach ([$bound, $unbound] as $reader) {
            try {
                $reader->bind([Column::text(1)]);
                $this->fail('a second plan was bound');
            } catch (\LogicException) {
                $this->assertCount(1, $reader->plan());
                $this->assertSame(0, $reader->plan()[0]->ordinal);
            }
        }
    }

    public function testARefusedPlanLeavesTheReaderUnbound(): void
    {
        $reader = DelimitedReader::fromString(self::ORDERS, Dialect::csv());
        try {
            $reader->bind([Column::i32(0), 'name']);
            $this->fail('a plan entry that is not a column was bound');
        } catch (\InvalidArgumentException) {
            $this->assertFalse($reader->isBound());
        }
        $reader->bind([Column::text(1)]);
        $this->assertSame(['alice', 'bob, jr', null], $reader->read()->values(0));
    }

    public function testAPlanHandedOverIsCheckedBeforeTheHeaderIsRead(): void
    {
        // The header is broken and the plan is too: the plan is the one refused.
        $this->expectException(\InvalidArgumentException::class);
        DelimitedReader::fromString("a,\"b\n1,2\n", Dialect::csv(), ['not a column']);
    }

    public function testABrokenHeaderIsRaisedWhenTheReaderIsOpenedWithoutAPlan(): void
    {
        try {
            DelimitedReader::fromString("a,\"b\n1,2\n", Dialect::csv());
            $this->fail('the unclosed header was read');
        } catch (TabularException $failure) {
            $this->assertSame(TabularFailure::UnclosedQuote, $failure->kind);
        }
    }

    public function testAStreamAndAFileOpenHeaderFirstToo(): void
    {
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, self::ORDERS);
        rewind($stream);
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, self::ORDERS);
        try {
            foreach (
                [
                    DelimitedReader::fromStream($stream, Dialect::csv(), bufferBytes: 3),
                    DelimitedReader::open($path, Dialect::csv(), bufferBytes: 3),
                ] as $reader
            ) {
                $reader->bind([Column::text($reader->headerIndex()->ordinal('name'))]);
                $this->assertSame(['alice', 'bob, jr', null], array_map(
                    static fn (Row $row): ?string => $row->value(0),
                    iterator_to_array($reader->rows())
                ));
                $reader->close();
            }
        } finally {
            fclose($stream);
            unlink($path);
        }
    }

    public function testEveryRowAnswersAsItsBatchDoes(): void
    {
        $reader = DelimitedReader::fromString(
            self::ORDERS,
            Dialect::csv(),
            [Column::i32(0), Column::text(1), Column::f64(2)]
        );
        $batch = $reader->read();
        $this->assertInstanceOf(\Traversable::class, $batch);
        $seen = 0;
        foreach ($batch as $index => $row) {
            $this->assertInstanceOf(Row::class, $row);
            $this->assertSame($seen++, $index);
            $this->assertRowIsItsBatch($batch, $row);
        }
        $this->assertSame($batch->rows(), $seen);

        $row = iterator_to_array($batch)[1];
        $this->assertEquals(new Success('bob, jr'), $row->get(1));
        $this->assertSame(3, $row->line());
        $this->assertSame('x', $row->raw(2));
        $this->assertNull($row->value(2));
        $this->assertSame(CastFailure::Malformed, $row->fault(2)->reason);
        $this->assertNull($row->fault(0));
        $this->expectException(\OutOfRangeException::class);
        $row->get(3);
    }

    public function testARowsReadSpansEveryBatchAndStopsAtTheEnd(): void
    {
        $csv = "n,half\n";
        for ($n = 1; $n <= 10; $n++) {
            $csv .= $n . ',' . ($n % 2 === 0 ? $n / 2 : 'odd') . "\n";
        }
        $reader = DelimitedReader::fromString($csv, Dialect::csv(), batchRows: 3);
        $header = $reader->headerIndex();
        $reader->bind([Column::i32($header->ordinal('n')), Column::i32($header->ordinal('half'))]);
        $rows = iterator_to_array($reader->rows());
        // Four batches — three of three rows and one of one — keyed as one run of rows.
        $this->assertSame(range(0, 9), array_keys($rows));
        $this->assertSame(range(1, 10), array_map(static fn (Row $row): int => $row->value(0), $rows));
        $this->assertSame(range(2, 11), array_map(static fn (Row $row): int => $row->line(), $rows));
        $this->assertSame([0, 1, 2, 0, 1, 2, 0, 1, 2, 0], array_map(static fn (Row $row): int => $row->index(), $rows));
        $this->assertInstanceOf(Fault::class, $rows[0]->get(1));
        $this->assertSame(5, $rows[9]->value(1));
        // Rows own what they show: the first is still good after the read has ended.
        $this->assertSame('odd', $rows[0]->raw(1));
        $this->assertNull($reader->read());
        $this->assertSame([], iterator_to_array($reader->rows()));
    }

    public function testASheetOpensHeaderFirst(): void
    {
        $book = Workbook::open(self::workbook('basic.xlsx'));
        $sheet = $book->sheet('Data', new SheetOptions(batchRows: 1));
        $this->assertFalse($sheet->isBound());
        $this->assertSame([], $sheet->plan());
        $header = $sheet->headerIndex();
        $this->assertSame($sheet->header(), $header->names());
        try {
            $sheet->read();
            $this->fail('an unbound sheet read');
        } catch (\LogicException) {
            $this->addToAssertionCount(1);
        }

        $plan = [Column::text($header->ordinal('name')), Column::text($header->ordinal('id'))];
        $sheet->bind($plan);
        $this->assertTrue($sheet->isBound());
        $this->assertSame($plan, $sheet->plan());
        try {
            $sheet->bind($plan);
            $this->fail('a second plan was bound');
        } catch (\LogicException) {
            $this->addToAssertionCount(1);
        }

        // The same rows as a sheet opened with that plan, a row at a time across batches.
        $planned = $book->sheet('Data', new SheetOptions(batchRows: 4), $plan);
        $expected = [];
        while (($batch = $planned->read()) !== null) {
            foreach ($batch as $row) {
                $this->assertRowIsItsBatch($batch, $row);
                $expected[] = [$row->line(), $row->get(0), $row->get(1), $row->raw(0)];
            }
        }
        $actual = [];
        foreach ($sheet->rows() as $row) {
            $actual[] = [$row->line(), $row->get(0), $row->get(1), $row->raw(0)];
        }
        // A batch a row: the read spans four batches.
        $this->assertCount(4, $actual);
        $this->assertEquals($expected, $actual);
        $this->assertNull($sheet->read());
    }

    public function testASheetWithoutAHeaderHasNoHeaderIndex(): void
    {
        $sheet = Workbook::open(self::workbook('basic.xlsx'))->sheet(0, new SheetOptions(hasHeader: false));
        $this->assertNull($sheet->header());
        $this->assertNull($sheet->headerIndex());
        $sheet->bind([Column::text(0)]);
        $this->assertSame('id', $sheet->read()->values(0)[0]);
    }

    public function testAWorkbookIsReadFromAStreamWhereItStands(): void
    {
        $bytes = file_get_contents(self::workbook('basic.xlsx'));
        $stream = fopen('php://temp', 'w+b');
        fwrite($stream, 'junk' . $bytes);
        fseek($stream, 4);
        $book = Workbook::fromStream($stream);
        $this->assertIsResource($stream);
        fclose($stream);
        $this->assertEquals(
            Workbook::fromString($bytes)->sheet(0, new SheetOptions())->headerIndex(),
            $book->sheet(0, new SheetOptions())->headerIndex()
        );
    }

    public function testAWorkbookStreamThatIsNotOneIsRefused(): void
    {
        $this->expectException(\InvalidArgumentException::class);
        Workbook::fromStream('basic.xlsx');
    }

    public function testAnEmptyStreamIsNotAWorkbook(): void
    {
        $stream = fopen('php://memory', 'r+b');
        try {
            $this->expectException(TabularException::class);
            Workbook::fromStream($stream);
        } finally {
            fclose($stream);
        }
    }

    /** Every member of a row is its batch's own answer for that row. */
    private function assertRowIsItsBatch(Batch $batch, Row $row): void
    {
        $index = $row->index();
        $this->assertSame($batch, $row->batch());
        $this->assertSame($batch->line($index), $row->line());
        foreach (array_keys($batch->columns()) as $column) {
            $this->assertEquals($batch->get($column, $index), $row->get($column));
            $this->assertSame($batch->values($column)[$index], $row->value($column));
            $this->assertSame($batch->faults($column)[$index] ?? null, $row->fault($column));
            $this->assertSame($batch->raw($column, $index), $row->raw($column));
        }
    }
}
