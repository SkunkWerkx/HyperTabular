<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use DateTimeImmutable;
use HyperCast\Cast;
use HyperCast\CastFailure;
use HyperCast\DateOrder;
use HyperCast\ExcelEpoch;
use HyperCast\Fault;
use HyperCast\NumFormat;
use HyperCast\Success;
use HyperCast\UnixPrecision;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Door;
use HyperTabular\TabularException;
use HyperTabular\TabularFailure;
use PHPUnit\Framework\TestCase;

/**
 * What the corpus does not reach: the binding's own surface and its caller bugs.
 */
final class ReaderTest extends TestCase
{
    private const ORDERS = "id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n3,,7\n";

    public function testAColumnIsAnArrayAndACellIsAUnion(): void
    {
        $reader = DelimitedReader::fromString(
            self::ORDERS,
            Dialect::csv(),
            [Column::i32(0), Column::text(1), Column::f64(2)]
        );
        $this->assertSame(['id', 'name', 'score'], $reader->header());
        $this->assertNotNull($batch = $reader->read());
        $this->assertSame([2, 3, 4], [$batch->line(0), $batch->line(1), $batch->line(2)]);
        $this->assertSame(3, $batch->rows());

        $this->assertSame([1, 2, 3], $batch->values(0));
        $this->assertSame([2.5, null, 7.0], $batch->values(2));
        $this->assertSame([], $batch->faults(0));
        $this->assertSame([1], array_keys($batch->faults(2)));
        $this->assertSame(CastFailure::Malformed, $batch->faults(2)[1]->reason);

        // The union, consumed the way HyperCast's own verdicts are.
        $verdict = $batch->get(2, 1);
        $described = match (true) {
            $verdict instanceof Success => (string) $verdict->value,
            $verdict instanceof Fault => "{$verdict->reason->name} in \"{$batch->raw(2, 1)}\"",
        };
        $this->assertSame('Malformed in "x"', $described);

        $this->assertEquals(new Success('bob, jr'), $batch->get(1, 1));
        $this->assertSame('bob, jr', $batch->raw(1, 1));
        // An empty cell is an Empty fault, which HyperCast's own optional() presents as null.
        $this->assertNull(Cast::optional($batch->get(1, 2)));
        $this->assertSame(['alice', 'bob, jr', null], $batch->values(1));
        $this->assertEquals(
            [new Success(1), new Success(2), new Success(3)],
            $batch->verdicts(0)
        );

        $this->assertNull($reader->read());
        // The batch owns what it shows: the reader having moved on changes none of it.
        $this->assertSame([1, 2, 3], $batch->values(0));
        $this->assertSame(4, $reader->records());
    }

    public function testEveryVerdictIsTheOneHyperCastGives(): void
    {
        // HyperCast is the judge: each cell through this reader is what HyperCast's own
        // door says of the same text, carrier and all. This binding needs HyperCast's types,
        // not its native library; where that library does not load there is nothing to ask.
        if (!Cast::isAvailable()) {
            $this->markTestSkipped("HyperCast's own native library did not load on this platform");
        }
        $cells = [
            'yes', '-12', '1,234', '(7)', '9e2', '200', '65535', '0xFFFFFFFF', '18446744073709551615',
            '2.5', '-1e-3', '79228162514264337593543950335', '6BA7B810-9DAD-11D1-80B4-00C04FD430C8',
            '2024-01-31T10:30:00.123456789+02:00', '1700000000123', '45292.75', '2024-02-29',
            '2/3/2026', '2026-01-07 15:04:05.5', '23:59:59.999999999', 'PT1H30M0.5S', ' as is ',
            '12x4', '256', '', '   ',
        ];
        $invariant = NumFormat::invariant();
        $doors = [
            [Column::bool(...), static fn (string $text) => Cast::bool($text)],
            [Column::i8(...), static fn (string $text) => Cast::i8($text, $invariant)],
            [Column::i16(...), static fn (string $text) => Cast::i16($text, $invariant)],
            [Column::i32(...), static fn (string $text) => Cast::i32($text, $invariant)],
            [Column::i64(...), static fn (string $text) => Cast::i64($text, $invariant)],
            [Column::u8(...), static fn (string $text) => Cast::u8($text, $invariant)],
            [Column::u16(...), static fn (string $text) => Cast::u16($text, $invariant)],
            [Column::u32(...), static fn (string $text) => Cast::u32($text, $invariant)],
            [Column::u64(...), static fn (string $text) => Cast::u64($text, $invariant)],
            [Column::f32(...), static fn (string $text) => Cast::f32($text, $invariant)],
            [Column::f64(...), static fn (string $text) => Cast::f64($text, $invariant)],
            [Column::decimal(...), static fn (string $text) => Cast::decimal($text, $invariant)],
            [Column::uuid(...), static fn (string $text) => Cast::uuid($text)],
            [Column::timestamp(...), static fn (string $text) => Cast::timestamp($text)],
            [
                static fn (int $ordinal) => Column::unix($ordinal, UnixPrecision::Milliseconds),
                static fn (string $text) => Cast::unix($text, UnixPrecision::Milliseconds),
            ],
            [
                static fn (int $ordinal) => Column::excelSerial($ordinal, ExcelEpoch::Y1900),
                static fn (string $text) => Cast::excelSerial($text, ExcelEpoch::Y1900),
            ],
            [Column::date(...), static fn (string $text) => Cast::date($text)],
            [
                static fn (int $ordinal) => Column::dateOrdered($ordinal, DateOrder::Mdy),
                static fn (string $text) => Cast::date($text, DateOrder::Mdy),
            ],
            [
                static fn (int $ordinal) => Column::datetime($ordinal, DateOrder::Ymd),
                static fn (string $text) => Cast::datetime($text, DateOrder::Ymd),
            ],
            [Column::time(...), static fn (string $text) => Cast::time($text)],
            [Column::duration(...), static fn (string $text) => Cast::duration($text)],
        ];
        // One column of input, every door reading it: a source column can be read through
        // more than one. Quoted, so a cell may hold the separator; blank lines are rows.
        $input = implode("\n", array_map(static fn (string $cell): string => "\"{$cell}\"", $cells)) . "\n";
        $plan = array_map(static fn (array $door): Column => $door[0](0), $doors);
        $plan[] = Column::text(0);
        $reader = DelimitedReader::fromString(
            $input,
            new Dialect(',', hasHeader: false, skipBlankLines: false),
            $plan,
            batchRows: 7
        );
        $this->assertCount(\count(Door::cases()), $plan);

        $row = 0;
        $cast = [];
        while (($batch = $reader->read()) !== null) {
            for ($index = 0; $index < $batch->rows(); $index++, $row++) {
                $text = $cells[$row];
                foreach ($doors as $column => [, $judge]) {
                    $expected = $judge($text);
                    $actual = $batch->get($column, $index);
                    $label = "'{$text}' through {$plan[$column]->door->name}";
                    $this->assertEquals($expected, $actual, $label);
                    if ($expected instanceof Success) {
                        $cast[$column] = true;
                        // assertEquals compares instants; the carrier must be the same too.
                        $this->assertSame(get_debug_type($expected->value), get_debug_type($actual->value), $label);
                        if ($expected->value instanceof DateTimeImmutable) {
                            $this->assertSame(
                                $expected->value->format('Y-m-d\TH:i:s.uP'),
                                $actual->value->format('Y-m-d\TH:i:s.uP'),
                                $label
                            );
                        }
                    }
                    $this->assertSame($text, $batch->raw($column, $index), $label);
                }
                // The text door: the bytes as they are, and Empty only for no bytes at all.
                $this->assertEquals(
                    $text === '' ? new Fault(CastFailure::Empty, 0, 0) : new Success($text),
                    $batch->get(\count($doors), $index)
                );
            }
        }
        $this->assertSame(\count($cells), $row);
        // The comparison was of values as well as of faults: every door cast something.
        $this->assertCount(\count($doors), $cast);
    }

    public function testAnEscapedCellIsItsUnescapedTextEverywhere(): void
    {
        $long = str_repeat('a ""quoted"" word, ', 600);
        $reader = DelimitedReader::fromString(
            "\"say \"\"hi\"\"\",\"12\"\"3\"\n\"{$long}\",\"7\"\n",
            new Dialect(',', hasHeader: false),
            [Column::text(0), Column::i32(1)],
            bufferBytes: 16
        );
        // A batch is the whole rows the buffer holds, and this buffer holds one at a time.
        $this->assertNotNull($batch = $reader->read());
        $this->assertSame(1, $batch->rows());
        $this->assertSame('say "hi"', $batch->values(0)[0]);
        $this->assertSame('say "hi"', $batch->raw(0, 0));
        // The fault's span indexes the unescaped text, which raw() gives back.
        $fault = $batch->get(1, 0);
        $this->assertInstanceOf(Fault::class, $fault);
        $this->assertSame('12"3', $batch->raw(1, 0));
        $this->assertSame('"', substr($batch->raw(1, 0), $fault->offset, $fault->length));
        // Larger than the arena and the scratch started out: both grew.
        $this->assertNotNull($batch = $reader->read());
        $this->assertSame(1, $batch->rows());
        $unescaped = str_replace('""', '"', $long);
        $this->assertSame($unescaped, $batch->values(0)[0]);
        $this->assertSame($unescaped, $batch->raw(0, 0));
        $this->assertEquals(new Success(7), $batch->get(1, 0));
        $this->assertNull($reader->read());
    }

    public function testAColumnPastTheRecordsEndReadsAsEmpty(): void
    {
        $reader = DelimitedReader::fromString(
            "a,b\n1,2\n",
            Dialect::csv(),
            [Column::i32(5), Column::text(1), Column::text(5)]
        );
        $this->assertNotNull($batch = $reader->read());
        $this->assertEquals(new Fault(CastFailure::Empty, 0, 0), $batch->get(0, 0));
        $this->assertSame('', $batch->raw(0, 0));
        $this->assertSame(['2'], $batch->values(1));
        $this->assertSame([null], $batch->values(2));
    }

    public function testAnEmptyPlanStillCountsRows(): void
    {
        $reader = DelimitedReader::fromString("a,b\n1,2\n3,4\n5,6\n", Dialect::csv(), [], batchRows: 2);
        $this->assertSame([], $reader->plan());
        $rows = 0;
        while (($batch = $reader->read()) !== null) {
            $rows += $batch->rows();
        }
        $this->assertSame(3, $rows);
        $this->assertSame(4, $reader->records());
    }

    public function testAskingForWhatIsNotThereIsACallerBug(): void
    {
        $reader = DelimitedReader::fromString("1,x\n", new Dialect(',', hasHeader: false), [Column::i32(0)]);
        $this->assertNull($reader->header());
        $this->assertNotNull($batch = $reader->read());
        foreach (
            [
                static fn () => $batch->get(0, 1),
                static fn () => $batch->get(0, -1),
                static fn () => $batch->get(1, 0),
                static fn () => $batch->values(1),
                static fn () => $batch->faults(-1),
                static fn () => $batch->verdicts(1),
                static fn () => $batch->raw(1, 0),
                static fn () => $batch->raw(0, 1),
            ] as $ask
        ) {
            try {
                $ask();
                $this->fail('expected an OutOfRangeException');
            } catch (\OutOfRangeException $e) {
                $this->addToAssertionCount(1);
            }
        }
    }

    public function testAnArenaThatCrampsABatchIsGrown(): void
    {
        // The core ends a batch early when its arena fills; the batch after one that did
        // starts with the arena doubled, so twenty thousand escaped rows are a handful of
        // batches, not thousands.
        $row = '"' . str_repeat('say ""hi"" ', 8) . "\"\n";
        $expected = str_repeat('say "hi" ', 8);
        $reader = DelimitedReader::fromString(
            str_repeat($row, 20_000),
            new Dialect(',', true, false),
            [Column::text(0)],
            DelimitedReader::DEFAULT_BATCH_ROWS,
            1 << 20
        );
        $batches = 0;
        $rows = 0;
        while (($batch = $reader->read()) !== null) {
            $batches++;
            $rows += $batch->rows();
            $this->assertSame($expected, $batch->values(0)[$batch->rows() - 1]);
        }
        $this->assertSame(20_000, $rows);
        $this->assertLessThan(15, $batches);
    }

    public function testAPlanOrADialectTheCoreCannotHonourIsRefusedUpFront(): void
    {
        $refused = [
            'a quote as the separator' => static fn () =>
                DelimitedReader::fromString(self::ORDERS, new Dialect('"'), [Column::i32(0)]),
            'a control byte as the separator' => static fn () =>
                DelimitedReader::fromString(self::ORDERS, new Dialect("\n"), [Column::i32(0)]),
            'a byte beyond ASCII as the separator' => static fn () =>
                DelimitedReader::fromString(self::ORDERS, new Dialect("\xE9"), [Column::i32(0)]),
            'a separator of more than one byte' => static fn () => new Dialect('é'),
            'no separator' => static fn () => new Dialect(''),
            'a plan entry that is not a column' => static fn () =>
                DelimitedReader::fromString(self::ORDERS, Dialect::csv(), [Column::i32(0), 'i32']),
            'a negative ordinal' => static fn () => Column::text(-1),
            'no rows a batch' => static fn () =>
                DelimitedReader::fromString(self::ORDERS, Dialect::csv(), [Column::i32(0)], batchRows: 0),
            'no buffer' => static fn () =>
                DelimitedReader::fromString(self::ORDERS, Dialect::csv(), [Column::i32(0)], bufferBytes: 0),
            'a stream that is not one' => static fn () =>
                DelimitedReader::fromStream('not a stream', Dialect::csv(), [Column::i32(0)]),
            // HyperCast's own NumFormat refuses a notation the core could not carry.
            'colliding separators' => static fn () => Column::f64(0, new NumFormat(',', ',', NumFormat::ALL)),
        ];
        foreach ($refused as $what => $build) {
            try {
                $build();
                $this->fail("{$what} was accepted");
            } catch (\InvalidArgumentException $e) {
                $this->addToAssertionCount(1);
            }
        }
    }

    public function testAStructuralFailureComesAfterTheIntactRowsAndIsFinal(): void
    {
        $reader = DelimitedReader::fromString("a,b\n1,2\n3,4\n5\n6,7\n", Dialect::csv(), [Column::i32(0)]);
        $this->assertNotNull($batch = $reader->read());
        $this->assertSame([1, 3], $batch->values(0));
        try {
            $reader->read();
            $this->fail('the short record was read');
        } catch (TabularException $failure) {
            $this->assertSame(TabularFailure::ColumnCount, $failure->kind);
            $this->assertSame(
                [3, 4, 12, 2, 1],
                [$failure->record, $failure->recordLine, $failure->byte, $failure->expected, $failure->found]
            );
            $this->assertSame(
                'Record 3 (line 4, byte 12) has 1 cells; the first record had 2.',
                $failure->getMessage()
            );
            try {
                $reader->read();
                $this->fail('a read after a structural failure succeeded');
            } catch (TabularException $again) {
                $this->assertSame($failure, $again);
            }
        }
    }

    public function testABrokenHeaderIsRaisedWhenTheReaderIsBuilt(): void
    {
        try {
            DelimitedReader::fromString("a,\"b\n1,2\n", Dialect::csv(), [Column::i32(0)]);
            $this->fail('the unclosed header was read');
        } catch (TabularException $failure) {
            $this->assertSame(TabularFailure::UnclosedQuote, $failure->kind);
            $this->assertSame([0, 0], [$failure->record, $failure->byte]);
        }
    }

    public function testAFileIsReadThroughTheSameReader(): void
    {
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        try {
            $file = fopen($path, 'wb');
            fwrite($file, "n\n");
            for ($row = 0; $row < 100_000; $row++) {
                fwrite($file, "{$row}\n");
            }
            fclose($file);

            $reader = DelimitedReader::open($path, Dialect::csv(), [Column::i64(0)], bufferBytes: 4096);
            $this->assertSame(['n'], $reader->header());
            $sum = 0;
            $rows = 0;
            $batches = 0;
            while (($batch = $reader->read()) !== null) {
                $sum += array_sum($batch->values(0));
                $rows += $batch->rows();
                $batches++;
            }
            $this->assertSame([100_000, 4_999_950_000], [$rows, $sum]);
            // A 4096-byte buffer holds far fewer rows than a batch could: one native call a refill.
            $this->assertGreaterThan(100, $batches);
            $this->assertSame(100_001, $reader->records());
            $reader->close();
            $reader->close();
            $this->expectException(\LogicException::class);
            $reader->read();
        } finally {
            unlink($path);
        }
    }

    public function testAFileThatIsNotThereIsAnError(): void
    {
        $this->expectException(\RuntimeException::class);
        $this->expectExceptionMessage('hypertabular: cannot open');
        DelimitedReader::open(sys_get_temp_dir() . '/hypertabular-no-such-file.csv', Dialect::csv(), []);
    }

    public function testAStreamStaysTheCallers(): void
    {
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, self::ORDERS);
        rewind($stream);
        $reader = DelimitedReader::fromStream($stream, Dialect::csv(), [Column::i32(0)], bufferBytes: 8);
        $ids = [];
        while (($batch = $reader->read()) !== null) {
            array_push($ids, ...$batch->values(0));
        }
        $this->assertSame([1, 2, 3], $ids);
        $reader->close();
        unset($reader);
        $this->assertIsResource($stream);
        $this->assertSame(\strlen(self::ORDERS), ftell($stream));
        fclose($stream);
    }

    public function testARecordLargerThanTheBufferGrowsIt(): void
    {
        $wide = str_repeat('x', 100_000);
        $reader = DelimitedReader::fromString(
            "a,b\n{$wide},1\n{$wide}{$wide},2\n",
            Dialect::csv(),
            [Column::text(0), Column::u8(1)],
            bufferBytes: 3
        );
        $this->assertSame(['a', 'b'], $reader->header());
        $lengths = [];
        $numbers = [];
        while (($batch = $reader->read()) !== null) {
            foreach ($batch->values(0) as $text) {
                $lengths[] = \strlen($text);
            }
            array_push($numbers, ...$batch->values(1));
        }
        $this->assertSame([100_000, 200_000], $lengths);
        $this->assertSame([1, 2], $numbers);
    }
}
