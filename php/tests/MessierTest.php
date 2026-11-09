<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use DateTimeImmutable;
use DateTimeZone;
use HyperCast\CastFailure;
use HyperCast\DateOptions;
use HyperCast\DateOrder;
use HyperCast\DetailedFault;
use HyperCast\Fault;
use HyperCast\FaultDetail;
use HyperCast\MonthNames;
use HyperCast\NumFormat;
use HyperCast\Success;
use HyperTabular\AmbiguousColumn;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Encoding;
use HyperTabular\LongRows;
use HyperTabular\Origin;
use HyperTabular\Problem;
use HyperTabular\Range;
use HyperTabular\SheetOptions;
use HyperTabular\ShortRows;
use HyperTabular\TabularException;
use HyperTabular\TabularFailure;
use HyperTabular\Tests\Support\Dribble;
use HyperTabular\Tests\Support\Package;
use HyperTabular\Tests\Support\Stalling;
use HyperTabular\Tests\Support\Throwing;
use HyperTabular\Workbook;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * The options a file past the plain case needs, each by every route a source takes, what a batch
 * says of its cells, and how long that stays right.
 */
final class MessierTest extends TestCase
{
    private static function workbooks(): string
    {
        return \dirname(__DIR__, 2) . '/corpus/workbook';
    }

    /** @return resource */
    private static function memory(string $bytes)
    {
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, $bytes);
        rewind($stream);
        return $stream;
    }

    private static function utc(string $date): DateTimeImmutable
    {
        return new DateTimeImmutable($date, new DateTimeZone('UTC'));
    }

    /**
     * What a read says, its rows as verdicts and origins and its faults counted across it, and
     * what it ended with: a stream may deliver the same rows in more batches.
     *
     * @param callable(): DelimitedReader $open
     */
    private static function said(callable $open): array
    {
        $rows = [];
        $faults = [];
        $ended = null;
        try {
            $held = $open();
            while (($batch = $held->read()) !== null) {
                foreach ($batch->faults() as $problem) {
                    $faults[] = [$problem->row + \count($rows), $problem->reason, $problem->detail];
                }
                for ($row = 0; $row < $batch->rows(); $row++) {
                    $cells = [];
                    foreach (array_keys($held->plan()) as $at) {
                        $cells[] = [$batch->get($at, $row), $batch->origin($at, $row)];
                    }
                    $rows[] = $cells;
                }
            }
        } catch (TabularException $failure) {
            $ended = [$failure->kind, $failure->record, $failure->recordLine, $failure->byte];
        }
        return [$rows, $faults, $ended];
    }

    /**
     * The same, from a string, a stream, a dribbling stream and a file, all the same or it fails;
     * the rows' values, and what it failed with.
     *
     * @param list<Column> $plan
     */
    private function everyWay(string $bytes, Dialect $dialect, array $plan): array
    {
        $first = self::said(static fn () => DelimitedReader::fromString($bytes, $dialect, $plan));
        $stream = self::memory($bytes);
        $this->assertEquals($first, self::said(static fn () => DelimitedReader::fromStream($stream, $dialect, $plan)));
        fclose($stream);
        Dribble::register();
        $stream = Dribble::open($bytes);
        $this->assertEquals(
            $first,
            self::said(static fn () => DelimitedReader::fromStream($stream, $dialect, $plan, bufferBytes: 1))
        );
        fclose($stream);
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, $bytes);
        try {
            $this->assertEquals($first, self::said(static fn () => DelimitedReader::open($path, $dialect, $plan)));
        } finally {
            unlink($path);
        }
        $rows = array_map(
            static fn (array $row): array => array_map(
                static fn (array $cell) => $cell[0] instanceof Success ? $cell[0]->value : $cell[0]->reason,
                $row
            ),
            $first[0]
        );
        return $first[2] === null ? $rows : [...$rows, $first[2][0]];
    }

    /** @return iterable<string, array{string, array<string, mixed>, list<Column>, list<mixed>, list<mixed>}> */
    public static function options(): iterable
    {
        $text = [Column::text(0)];
        $malformed = CastFailure::Malformed;
        yield 'skip' => ["title\na\n7\n", ['skip' => 1], [Column::i32(0)], [[7]], [[$malformed], [7]]];
        yield 'headerRow' => ["x\na\n7\n", ['headerRow' => 1], [Column::i32(0)], [[7]], [[$malformed], [7]]];
        yield 'shortRows' => [
            "a,b\n7\n", ['shortRows' => ShortRows::Pad], [Column::i32(0), Column::i32(1)], [[7, CastFailure::Empty]],
            [TabularFailure::ColumnCount],
        ];
        yield 'longRows report' => [
            "a\n7,8\n",
            ['longRows' => LongRows::Report],
            [Column::i32(0)],
            [[7]],
            [TabularFailure::ColumnCount]
        ];
        yield 'longRows truncate' => [
            "a\n7,8\n",
            ['longRows' => LongRows::Truncate],
            [Column::i32(0)],
            [[7]],
            [TabularFailure::ColumnCount]
        ];
        yield 'windows-1252' => ["a\n\x80\n", ['encoding' => Encoding::Windows1252], $text, [['€']], [[$malformed]]];
        yield 'latin-1' => ["a\n\xE9\n", ['encoding' => Encoding::Latin1], $text, [['é']], [[$malformed]]];
        yield 'utf-16le' => [
            mb_convert_encoding("a\n€\n", 'UTF-16LE', 'UTF-8'), ['encoding' => Encoding::Utf16Le], $text, [['€']],
            [[$malformed], ["\0"]],
        ];
        yield 'tokens' => [
            "a\nNA\n7\n",
            ['tokens' => ['NA']],
            [Column::i32(0)],
            [[CastFailure::Empty], [7]],
            [[$malformed], [7]]
        ];
    }

    /**
     * @param array<string, mixed> $declared
     * @param list<Column> $plan
     */
    #[DataProvider('options')]
    public function testEachOptionDoesItsOwnWorkByEveryRoute(
        string $bytes,
        array $declared,
        array $plan,
        array $with,
        array $without,
    ): void {
        $this->assertSame($with, $this->everyWay($bytes, new Dialect(',', ...$declared), $plan));
        $this->assertSame($without, $this->everyWay($bytes, Dialect::csv(), $plan));
    }

    public function testADialectRefusesWhatItCannotBeTold(): void
    {
        foreach (
            [['skip' => -1], ['headerRow' => -1], ['skip' => 1 << 32], ['headerRow' => 1 << 32], ['tokens' => [1]],
                ['tokens' => ["\xFF"]]] as $declared
        ) {
            try {
                new Dialect(',', ...$declared);
                $this->fail(json_encode(array_keys($declared)) . ' taken');
            } catch (\InvalidArgumentException) {
                $this->addToAssertionCount(1);
            }
        }
        foreach ([['skip' => 1.5], ['tokens' => 'NA'], ['encoding' => 'utf-8'], ['shortRows' => 1]] as $declared) {
            try {
                new Dialect(',', ...$declared);
                $this->fail(json_encode(array_keys($declared)) . ' taken');
            } catch (\TypeError) {
                $this->addToAssertionCount(1);
            }
        }
    }

    public function testAHeaderRowPastTheInputIsNoHeader(): void
    {
        $this->assertSame([], DelimitedReader::fromString("x\n", new Dialect(',', headerRow: 5))->header());
    }

    public function testTheLinesABatchSaysCountWhatWasPassedOver(): void
    {
        $batch = DelimitedReader::fromString(
            "title\nx\na\n7\n8\n",
            new Dialect(',', skip: 1, headerRow: 1),
            [Column::i32(0)]
        )->read();
        $this->assertSame([4, 5], [$batch->line(0), $batch->line(1)]);
        $this->assertSame([7, 8], $batch->values(0));
    }

    public function testAColumnsOwnTokensReplaceTheDialects(): void
    {
        $text = "a,b,c\nNA,NA,-\n";
        $dialect = new Dialect(',', tokens: ['NA']);
        $batch = DelimitedReader::fromString(
            $text,
            $dialect,
            [Column::i32(0), Column::i32(1)->withTokens(['-']), Column::i32(2)->withTokens(['-'])]
        )->read();
        $this->assertSame([Origin::Token, CastFailure::Empty], [$batch->origin(0, 0), $batch->get(0, 0)->reason]);
        $this->assertSame(CastFailure::Malformed, $batch->get(1, 0)->reason);
        $this->assertSame(Origin::Token, $batch->origin(2, 0));
        // An empty list says this column has none, the dialect's included.
        $batch = DelimitedReader::fromString($text, $dialect, [Column::i32(0)->withTokens([]), Column::i32(1)])->read();
        $this->assertSame(
            [CastFailure::Malformed, Origin::Cell, Origin::Token],
            [$batch->get(0, 0)->reason, $batch->origin(0, 0), $batch->origin(1, 0)]
        );
        // Not said is not none.
        $this->assertNull(Column::i32(0)->tokens);
        $this->assertSame([], Column::i32(0)->withTokens([])->tokens);
        $this->assertSame(['-'], Column::i32(0)->withTokens(['-'])->tokens);
        foreach ([[1], ["\xFF"]] as $tokens) {
            try {
                DelimitedReader::fromString($text, $dialect, [Column::i32(0)->withTokens($tokens)]);
                $this->fail('a token that is not a UTF-8 string was taken');
            } catch (\InvalidArgumentException) {
                $this->addToAssertionCount(1);
            }
        }
    }

    public function testAQuotedCellIsNeverAToken(): void
    {
        $batch = DelimitedReader::fromString(
            "a,b\nNA,\"NA\"\n",
            new Dialect(',', tokens: ['NA']),
            [Column::text(0), Column::text(1)]
        )
            ->read();
        $this->assertSame([Origin::Token, CastFailure::Empty], [$batch->origin(0, 0), $batch->get(0, 0)->reason]);
        $this->assertEquals([Origin::Cell, new Success('NA')], [$batch->origin(1, 0), $batch->get(1, 0)]);
    }

    public function testAColumnReadsItsDateOptionsAndMonthNames(): void
    {
        $compact = new DateOptions(DateOptions::COMPACT_DATE);
        $text = "d\n20260207\n";
        $this->assertSame(
            CastFailure::Malformed,
            DelimitedReader::fromString($text, Dialect::csv(), [Column::date(0)])->read()->get(0, 0)->reason
        );
        $declared = Column::date(0)->withDateOptions($compact);
        $this->assertEquals(
            [self::utc('2026-02-07')],
            DelimitedReader::fromString($text, Dialect::csv(), [$declared])->read()->values(0)
        );
        $this->assertSame([$compact, null], [$declared->dateOptions, $declared->months]);

        $names = ['janv', 'fevr', 'mars', 'avr', 'mai', 'juin', 'juil', 'aout', 'sept', 'oct', 'nov', 'dec'];
        $months = new MonthNames(array_combine(range(1, 12), $names));
        $text = "d\n2-fevr-2026\n";
        $plain = Column::dateOrdered(0, DateOrder::Dmy);
        $this->assertSame(
            CastFailure::Malformed,
            DelimitedReader::fromString($text, Dialect::csv(), [$plain])->read()->get(0, 0)->reason
        );
        $named = $plain->withDateOptions(new DateOptions(), $months);
        $this->assertSame($months, $named->months);
        $this->assertEquals(
            [self::utc('2026-02-02')],
            DelimitedReader::fromString($text, Dialect::csv(), [$named])->read()->values(0)
        );
        // A column is a value: what it was made from is as it was.
        $this->assertNull($plain->dateOptions);
    }

    public function testWhatFailedIsOnTheDetailAndTheList(): void
    {
        $batch = DelimitedReader::fromString("a,b\n1x,300\n", Dialect::csv(), [Column::i32(0), Column::u8(1)])->read();
        $malformed = $batch->detail(0, 0);
        $outOfRange = $batch->detail(1, 0);
        $this->assertInstanceOf(DetailedFault::class, $malformed);
        $this->assertSame(FaultDetail::UnexpectedCharacter, $malformed->detail);
        $this->assertEquals($batch->get(0, 0), $malformed->toFault());
        $this->assertSame(CastFailure::OutOfRange, $outOfRange?->reason);
        [$first, $second] = $batch->faults();
        $this->assertInstanceOf(Problem::class, $first);
        $this->assertSame([$malformed->detail, $malformed->code], [$first->detail, $first->detailCode]);
        $this->assertSame([$outOfRange->detail, CastFailure::OutOfRange], [$second->detail, $second->reason]);
        // The same by the row.
        foreach ($batch as $row) {
            $this->assertEquals($malformed, $row->detail(0));
            $this->assertSame(Origin::Cell, $row->origin(0));
        }
        // A cell that cast has none.
        $this->assertNull(
            DelimitedReader::fromString("a\n1\n", Dialect::csv(), [Column::i32(0)])->read()->detail(0, 0)
        );
    }

    public function testAFaultsSpanCountsBytes(): void
    {
        // Two two-byte digits, then the character the core stops at: byte 4.
        $arabic = new NumFormat('.', ',', NumFormat::ALL, digitZero: '٠');
        $batch = DelimitedReader::fromString("a\n٠٠x\n", Dialect::csv(), [Column::i32(0, $arabic)])->read();
        $verdict = $batch->get(0, 0);
        $problem = $batch->faults()[0];
        $this->assertSame([4, 1], [$verdict->offset, $verdict->length]);
        $this->assertSame([4, 1], [$problem->offset, $problem->length]);
        $this->assertSame('x', substr($batch->raw(0, 0), $verdict->offset, $verdict->length));
    }

    public function testNotesOffSayNothingOfAFaultButItsReason(): void
    {
        $plan = [Column::i32(0)];
        $stream = self::memory("a\n1x\n");
        foreach (
            [DelimitedReader::fromString("a\n1x\n", Dialect::csv(), $plan, notes: false),
                DelimitedReader::fromStream($stream, Dialect::csv(), $plan, notes: false)] as $reader
        ) {
            $batch = $reader->read();
            $this->assertSame(CastFailure::Malformed, $batch->get(0, 0)->reason);
            $this->assertNull($batch->detail(0, 0));
            $problem = $batch->faults()[0];
            $this->assertSame(
                [null, 0, Origin::Cell, Origin::Cell],
                [$problem->detail, $problem->detailCode, $problem->origin, $batch->origin(0, 0)]
            );
        }
        fclose($stream);
        // A token is an empty cell, and no more is said.
        $batch = DelimitedReader::fromString("a\nNA\n", new Dialect(',', tokens: ['NA']), $plan, notes: false)->read();
        $this->assertSame(
            [CastFailure::Empty, Origin::Cell, []],
            [$batch->get(0, 0)->reason, $batch->origin(0, 0), $batch->faults()]
        );
        // And the values are the same.
        $text = "a,b\n1,x\n2,y\n";
        $plan = [Column::i32(0), Column::text(1)];
        $on = DelimitedReader::fromString($text, Dialect::csv(), $plan)->read();
        $off = DelimitedReader::fromString($text, Dialect::csv(), $plan, notes: false)->read();
        $this->assertSame([$on->values(0), $on->values(1)], [$off->values(0), $off->values(1)]);
        $this->assertSame([[], []], [$on->faults(), $off->faults()]);
    }

    public function testNotesIsItsOwnArgumentOnEveryWayToOpen(): void
    {
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, "a\nx\n");
        try {
            $this->assertNull(
                DelimitedReader::open($path, Dialect::csv(), [Column::i32(0)], notes: false)->read()->detail(0, 0)
            );
            $this->assertNotNull(DelimitedReader::open($path, Dialect::csv(), [Column::i32(0)])->read()->detail(0, 0));
        } finally {
            unlink($path);
        }
        $this->expectException(\RuntimeException::class);
        @DelimitedReader::open('/no/such.csv', Dialect::csv(), notes: false);
    }

    public function testAPaddedCellIsABlankAndALongRowIsListed(): void
    {
        $dialect = new Dialect(',', shortRows: ShortRows::Pad, longRows: LongRows::Report);
        $batch = DelimitedReader::fromString(
            "a,b\n1\n2,3,4\n5,6\n",
            $dialect,
            [Column::i32(0), Column::i32(1)]
        )->read();
        $this->assertSame([Origin::Padded, CastFailure::Empty], [$batch->origin(1, 0), $batch->get(1, 0)->reason]);
        $this->assertCount(1, $batch->faults());
        $long = $batch->faults()[0];
        $this->assertSame([1, null, null], [$long->row, $long->planColumn, $long->sourceColumn]);
        $this->assertSame([2, 3, 3], [$long->expected, $long->found, $long->line]);
        $this->assertSame([null, null, Origin::Cell], [$long->reason, $long->detail, $long->origin]);
    }

    public function testAProblemNamesItsSourceColumnAsWellAsItsPlans(): void
    {
        $problem = DelimitedReader::fromString("a,b,c\n1,2,x\n", Dialect::csv(), [Column::i32(2)])->read()->faults()[0];
        $this->assertSame(
            [0, 2, 0, 1],
            [$problem->planColumn, $problem->sourceColumn, $problem->offset, $problem->length]
        );
        // Asked for one column, the faults are 1.0's map of that column's.
        $faults = DelimitedReader::fromString("a,b,c\n1,2,x\n", Dialect::csv(), [Column::i32(2)])->read()->faults(0);
        $this->assertEquals([0 => new Fault(CastFailure::Malformed, 0, 1)], $faults);
    }

    public function testARangeIsAValue(): void
    {
        $made = new Range(4, 2, 12, 3);
        $this->assertSame([4, 2, 12, 3], [$made->firstRow, $made->firstColumn, $made->lastRow, $made->lastColumn]);
        $this->assertEquals($made, Range::parse('B4:C12'));
        $this->assertNotEquals($made, Range::parse('B4:C13'));
        $this->assertEquals(new Range(), new Range(0, 0, 0, 0));
        foreach (['', 'A', '1', 'A1', 'B4:', 'C12:B4', 'A0:B1'] as $text) {
            try {
                Range::parse($text);
                $this->fail("{$text} parsed");
            } catch (\InvalidArgumentException $refused) {
                $this->assertSame('not a range: ' . json_encode($text), $refused->getMessage());
            }
        }
        foreach ([[-1], [5, 0, 4], [1 << 32]] as $corners) {
            try {
                new Range(...$corners);
                $this->fail(json_encode($corners) . ' made');
            } catch (\InvalidArgumentException) {
                $this->addToAssertionCount(1);
            }
        }
        // 0 is no limit, so a first with no last is every row from there.
        $this->assertSame(0, (new Range(5))->lastRow);
        $this->expectException(\Error::class);
        $made->firstRow = 1;
    }

    public function testASheetsRangeIsWhatItReads(): void
    {
        $book = Workbook::open(self::workbooks() . '/ranges.xlsx');
        $whole = $book->sheet(0, new SheetOptions())->suggest();
        $ranged = $book->sheet(0, new SheetOptions(range: Range::parse('B4:C12')));
        $found = $ranged->suggest();
        $this->assertCount(2, $found->plan);
        $this->assertNotCount(2, $whole->plan);
        $this->assertSame('id', $found->report[0]->name);
        $this->assertSame(['id', $found->report[1]->name], $ranged->header());
        $sheet = $book->sheet(0, new SheetOptions(range: Range::parse('B4:C12')), [Column::i32(0), Column::text(1)]);
        $listed = [];
        while (($batch = $sheet->read()) !== null) {
            foreach ($batch->faults() as $problem) {
                $listed[] = [$problem->sourceColumn, $problem->planColumn, $batch->line($problem->row) > 4];
            }
        }
        $this->assertNotEmpty($listed);
        $this->assertSame(array_fill(0, \count($listed), [1, 0, true]), $listed);
        // A batch's last column is the last it asked for, in a range ending at it.
        $sheet = $book->sheet(0, new SheetOptions(range: Range::parse('B4:C12')));
        $this->assertCount(2, $sheet->header());
        $sheet->bind([Column::text(1), Column::text(2)]);
        $this->assertSame([null], array_values(array_unique($sheet->read()->values(1))));
    }

    public function testASheetsSkipHeaderRowAndTokensChangeWhatItReads(): void
    {
        $book = Workbook::open(self::workbooks() . '/basic.xlsx');
        $plan = [Column::text(1)];
        $all = static function ($sheet): array {
            $values = [];
            while (($batch = $sheet->read()) !== null) {
                array_push($values, ...$batch->values(0));
            }
            return $values;
        };
        $plain = $book->sheet(0, new SheetOptions(), $plan);
        $this->assertNotNull($plain->header());
        $rows = $all($plain);
        $skipped = $book->sheet(0, new SheetOptions(skip: 1), $plan);
        $this->assertSame($rows[0], $skipped->header()[1]);
        $this->assertSame(\array_slice($rows, 1), $all($skipped));
        $headed = $book->sheet(0, new SheetOptions(headerRow: 1), $plan);
        $this->assertSame($skipped->header(), $headed->header());
        $this->assertSame(\array_slice($rows, 1), $all($headed));
        $batch = $book->sheet(0, new SheetOptions(tokens: [$rows[0]]), $plan)->read();
        $this->assertSame([CastFailure::Empty, Origin::Token], [$batch->get(0, 0)->reason, $batch->origin(0, 0)]);
        // Notes off, a sheet has no origin to say.
        $batch = $book->sheet(0, new SheetOptions(tokens: [$rows[0]], notes: false), $plan)->read();
        $this->assertSame([CastFailure::Empty, Origin::Cell], [$batch->get(0, 0)->reason, $batch->origin(0, 0)]);
        foreach ([['skip' => -1], ['headerRow' => -1], ['skip' => 1 << 32]] as $declared) {
            try {
                new SheetOptions(...$declared);
                $this->fail(json_encode($declared) . ' taken');
            } catch (\InvalidArgumentException) {
                $this->addToAssertionCount(1);
            }
        }
    }

    /**
     * With `skipEmptyRows: false` a sheet keeps every row, a formula with no cached value among
     * them: what a read that wants every row has to ask for.
     */
    public function testSkipEmptyRowsOffKeepsEveryRowAFormulaWithNoCachedValueAmongThem(): void
    {
        $book = Workbook::open(self::workbooks() . '/uncached.xlsx');
        $plan = [Column::text(0), Column::i32(1)];
        $batches = static function (SheetOptions $options) use ($book, $plan): array {
            $sheet = $book->sheet(0, $options, $plan);
            $read = [];
            while (($batch = $sheet->read()) !== null) {
                $read[] = $batch;
            }
            return $read;
        };
        $rows = static fn (array $read): int => array_sum(
            array_map(static fn (Batch $batch): int => $batch->rows(), $read)
        );
        $kept = $batches(new SheetOptions(skipEmptyRows: false));
        $dropped = $batches(new SheetOptions(skipEmptyRows: true));
        $this->assertSame($rows($dropped) + 2, $rows($kept));
        $origins = [];
        foreach ($kept as $batch) {
            for ($row = 0; $row < $batch->rows(); $row++) {
                $origins[] = $batch->origin(1, $row);
            }
        }
        $this->assertContains(Origin::Uncached, $origins);
        $this->assertContains(
            Origin::Uncached,
            array_map(static fn (Problem $problem): Origin => $problem->origin, $kept[0]->faults())
        );
        // With notes off, the list can't say a formula has no cached value.
        $batch = $book->sheet(0, new SheetOptions(skipEmptyRows: false, notes: false), $plan)->read();
        $this->assertNotContains(
            Origin::Uncached,
            array_map(static fn (Problem $problem): Origin => $problem->origin, $batch->faults())
        );
        $this->assertSame(Origin::Cell, $batch->origin(1, 0));
    }

    public function testAStringLongerThanTheWindowReadsFromADeflatedSheet(): void
    {
        $long = str_repeat('x', 256 * 1024);
        $package = Package::xlsx("<row r=\"1\"><c t=\"inlineStr\"><is><t>{$long}</t></is></c></row>");
        $batch = Workbook::fromString($package)->sheet(0, new SheetOptions(false), [Column::text(0)])->read();
        $this->assertSame([$long], $batch->values(0));
    }

    public function testAnEncodingFailureComesAfterEveryIntactRowWithTheFilesByte(): void
    {
        $dialect = new Dialect(',', encoding: Encoding::Utf16Le);
        $bytes = mb_convert_encoding("a\n1\n2\n", 'UTF-16LE', 'UTF-8') . "\x00\xDC\n\x00";
        Dribble::register();
        foreach (['string', 'stream', 'dribble'] as $route) {
            $source = match ($route) {
                'string' => null,
                'stream' => self::memory($bytes),
                'dribble' => Dribble::open($bytes),
            };
            $held = $source === null
                ? DelimitedReader::fromString($bytes, $dialect, [Column::i32(0)])
                : DelimitedReader::fromStream($source, $dialect, [Column::i32(0)]);
            $seen = [];
            try {
                foreach ($held->rows() as $row) {
                    $seen[] = $row->value(0);
                }
                $this->fail("{$route}: no failure");
            } catch (TabularException $failure) {
                $this->assertSame(
                    [TabularFailure::Encoding, 3, 4, 12],
                    [$failure->kind, $failure->record, $failure->recordLine, $failure->byte],
                    $route
                );
            }
            $this->assertSame([1, 2], $seen, $route);
            // Final: the same one, again.
            try {
                $held->read();
                $this->fail("{$route}: read on");
            } catch (TabularException $again) {
                $this->assertSame($failure, $again, $route);
            }
            if ($source !== null) {
                fclose($source);
            }
        }
        // A held carriage return is finished before the failure says so.
        $bytes = mb_convert_encoding("a\n1\r", 'UTF-16LE', 'UTF-8') . "\x00\xDC";
        $held = DelimitedReader::fromString($bytes, $dialect, [Column::i32(0)]);
        $this->assertSame([1], $held->read()->values(0));
        $this->expectException(TabularException::class);
        $held->read();
    }

    public function testAFailureInTheHeaderIsSaidAtOpeningWithTheFilesByte(): void
    {
        $dialect = new Dialect(',', encoding: Encoding::Utf16Le);
        $bytes = "\xFF\xFEa\x00,\x00\x00\xD8";
        Dribble::register();
        foreach ([null, self::memory($bytes), Dribble::open($bytes)] as $source) {
            try {
                $source === null
                    ? DelimitedReader::fromString($bytes, $dialect)
                    : DelimitedReader::fromStream($source, $dialect);
                $this->fail('a broken header opened');
            } catch (TabularException $failure) {
                $this->assertSame([TabularFailure::Encoding, 6], [$failure->kind, $failure->byte]);
            }
        }
    }

    public function testAnEncodedSourceReadsByAMarkWithACharacterCutAcrossReads(): void
    {
        Dribble::register();
        $bytes = mb_convert_encoding("\u{FEFF}a\n€\n🙂\n", 'UTF-16LE', 'UTF-8');
        $stream = Dribble::open($bytes);
        $held = DelimitedReader::fromStream(
            $stream,
            new Dialect(',', encoding: Encoding::Utf16Le),
            [Column::text(0)],
            bufferBytes: 1
        );
        $this->assertSame(['a'], $held->header());
        $values = [];
        foreach ($held->rows() as $row) {
            $values[] = $row->value(0);
        }
        $this->assertSame(['€', '🙂'], $values);
        fclose($stream);
    }

    /** @return iterable<string, array{Encoding, string}> */
    public static function encodings(): iterable
    {
        yield 'utf-8' => [Encoding::Utf8, 'UTF-8'];
        yield 'latin-1' => [Encoding::Latin1, 'ISO-8859-1'];
        yield 'windows-1252' => [Encoding::Windows1252, 'Windows-1252'];
        yield 'utf-16le' => [Encoding::Utf16Le, 'UTF-16LE'];
    }

    /**
     * A stream that hands over its transfers and then has nothing until it's let go, as a pipe or
     * a socket does: a read past them fails the test, which is how a call that would have waited
     * for more says so here.
     */
    #[DataProvider('encodings')]
    public function testAStalledStreamsFinishedRowsComeToTheCaller(Encoding $encoding, string $codec): void
    {
        $dialect = new Dialect(',', encoding: $encoding);
        $encode = static fn (string $text): string => mb_convert_encoding($text, $codec, 'UTF-8');
        Stalling::register();
        $stream = Stalling::open([$encode("a\n1\n2\n"), $encode("3\n")]);
        $held = DelimitedReader::fromStream($stream, $dialect, [Column::i32(0)]);
        $this->assertSame(['a'], $held->header());
        $this->assertSame(1, Stalling::$reads);
        $batch = $held->read();
        $this->assertSame([1, 2], \array_slice($batch->values(0), 0, 2));
        $this->assertLessThanOrEqual(2, Stalling::$reads);
        $seen = $batch->rows();
        while ($seen < 3) {
            $seen += $held->read()->rows();
        }
        Stalling::$released = true;
        $this->assertNull($held->read());
        fclose($stream);

        // The rows of the one transfer after a header's.
        $stream = Stalling::open([$encode("a\n"), $encode("1\n2\n")]);
        $held = DelimitedReader::fromStream($stream, $dialect, [Column::i32(0)]);
        $this->assertSame([1, 2], $held->read()->values(0));
        $this->assertSame(2, Stalling::$reads);
        Stalling::$released = true;
        $this->assertNull($held->read());
        fclose($stream);
    }

    /**
     * A socket with two rows in it and its writer still open: a read that waited for a full buffer
     * would wait out the socket's timeout, which is what is timed.
     */
    public function testAPipesFinishedRowsComeToTheCaller(): void
    {
        [$readable, $writable] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
        stream_set_timeout($readable, 3);
        fwrite($writable, "a\n1\n2\n");
        $started = hrtime(true);
        $held = DelimitedReader::fromStream($readable, Dialect::csv(), [Column::i32(0)]);
        $batch = $held->read();
        $elapsed = (hrtime(true) - $started) / 1e9;
        $this->assertSame([1, 2], $batch->values(0));
        $this->assertLessThan(1.5, $elapsed, 'it waited on the socket with finished rows in hand');
        fclose($writable);
        $this->assertNull($held->read());
        fclose($readable);
    }

    /** What a stream throws goes to the caller, and isn't final: the read asks again. */
    #[DataProvider('encodings')]
    public function testAStreamsErrorGoesToTheCallerAndTheReadCarriesOn(Encoding $encoding, string $codec): void
    {
        $encode = static fn (string $text): string => mb_convert_encoding($text, $codec, 'UTF-8');
        Throwing::register();
        $stream = Throwing::open([$encode("a\n1\n"), new \RuntimeException('the wire went'), $encode("2\n")]);
        $held = DelimitedReader::fromStream($stream, new Dialect(',', encoding: $encoding), [Column::i32(0)]);
        $this->assertSame([1], $held->read()->values(0));
        try {
            $held->read();
            $this->fail('the stream\'s error was not raised');
        } catch (\RuntimeException $raised) {
            $this->assertNotInstanceOf(TabularException::class, $raised);
            $this->assertSame('the wire went', $raised->getMessage());
        }
        $this->assertSame([2], $held->read()->values(0));
        $this->assertNull($held->read());
        fclose($stream);
    }

    /**
     * A record cut by the error, longer than the first record of what was read ahead of it: the
     * read has text it has handed over behind it, and text it hasn't in front, when the stream
     * throws, twice.
     */
    #[DataProvider('encodings')]
    public function testAReadStandsWhereItWasWhenAStreamThrowsWithARecordPending(
        Encoding $encoding,
        string $codec
    ): void {
        $encode = static fn (string $text): string => mb_convert_encoding($text, $codec, 'UTF-8');
        Throwing::register();
        $wire = new \RuntimeException('the wire went');
        $stream = Throwing::open([$encode("h\nr1\nr2\nr3\nr4\nlongpartial"), $wire, $wire, $encode("tail\nr6\n")]);
        $held = DelimitedReader::fromStream($stream, new Dialect(',', encoding: $encoding), [Column::text(0)], 2);
        $rows = [];
        $raised = 0;
        while (true) {
            try {
                $batch = $held->read();
            } catch (\RuntimeException $thrown) {
                $this->assertSame($wire, $thrown);
                $raised++;
                continue;
            }
            if ($batch === null) {
                break;
            }
            for ($row = 0; $row < $batch->rows(); $row++) {
                $rows[] = [$batch->line($row), $batch->values(0)[$row]];
            }
        }
        $this->assertSame(2, $raised);
        $this->assertSame([[2, 'r1'], [3, 'r2'], [4, 'r3'], [5, 'r4'], [6, 'longpartialtail'], [7, 'r6']], $rows);
        $this->assertNull($held->read());
        fclose($stream);
    }

    public function testABatchOutlivesItsReaderAndTheNextRead(): void
    {
        $text = "a,b\n1x,y\n" . implode(
            '',
            array_map(static fn (int $row): string => "{$row},z\n", range(0, 2))
        ) . "2x,w\n";
        $held = DelimitedReader::fromString($text, Dialect::csv(), [Column::i32(0), Column::text(1)], 1);
        $first = $held->read();
        for ($i = 0; $i < 3; $i++) {
            $held->read();
        }
        $held->close();
        unset($held);
        gc_collect_cycles();
        $problem = $first->faults()[0];
        $this->assertSame(FaultDetail::UnexpectedCharacter, $first->detail(0, 0)->detail);
        $this->assertSame(
            ['1x', 'y', Origin::Cell],
            [$first->raw(0, 0), $first->get(1, 0)->value, $first->origin(0, 0)]
        );
        $this->assertSame([0, 2], [$problem->row, $problem->line]);
        // And a batch's list is its own: the same each time it is asked.
        $this->assertEquals($first->faults(), $first->faults());
    }

    public function testABatchAnswersForItselfWhicheverIsReadSince(): void
    {
        $held = DelimitedReader::fromString(
            "a,b\n1,x\ny,2\n3,4\n",
            Dialect::csv(),
            [Column::i32(0), Column::i32(1)],
            1
        );
        $first = $held->read();
        $second = $held->read();
        $third = $held->read();
        $shape = static fn (Batch $batch): array => array_map(
            static fn (Problem $problem): array => [$problem->row, $problem->line, $problem->planColumn],
            $batch->faults()
        );
        $this->assertSame([], $shape($third));
        $this->assertSame([[0, 3, 0]], $shape($second));
        $this->assertSame([[0, 2, 1]], $shape($first));
    }

    public function testASheetsBatchOutlivesItsWorkbookWithItsNotes(): void
    {
        $book = Workbook::open(self::workbooks() . '/ranges.xlsx');
        $sheet = $book->sheet(0, new SheetOptions(), [Column::i32(1)]);
        $batch = $sheet->read();
        $said = $batch->faults();
        unset($book, $sheet);
        gc_collect_cycles();
        $this->assertEquals($said, $batch->faults());
    }

    public function testAClosedReaderSaysSoAndABatchStillReads(): void
    {
        $held = DelimitedReader::fromString("a\nx\n", Dialect::csv(), [Column::i32(0)]);
        $batch = $held->read();
        $held->close();
        try {
            $held->read();
            $this->fail('a closed reader read');
        } catch (\LogicException $closed) {
            $this->assertSame('The reader is closed', $closed->getMessage());
        }
        $this->assertSame(CastFailure::Malformed, $batch->faults()[0]->reason);
    }

    public function testAHeaderThatNamesAColumnTwiceIsReadByPosition(): void
    {
        $held = DelimitedReader::fromString("id,x,id\n1,2,3\n", Dialect::csv());
        try {
            $held->headerIndex()->ordinal('id');
            $this->fail('an ambiguous name was resolved');
        } catch (AmbiguousColumn $twice) {
            $this->assertSame(['id', 0, 2], [$twice->name, $twice->first, $twice->second]);
        }
        $this->assertSame(0, $held->headerIndex()->find('id'));
        $this->assertSame(1, $held->headerIndex()->ordinal('x'));
        $held->bind([Column::i32(2)]);
        $this->assertSame([3], $held->read()->values(0));
    }
}
