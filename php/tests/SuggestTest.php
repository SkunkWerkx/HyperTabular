<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperCast\CastFailure;
use HyperCast\DateOrder;
use HyperCast\NumFormat;
use HyperCast\Success;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Door;
use HyperTabular\Encoding;
use HyperTabular\LongRows;
use HyperTabular\Range;
use HyperTabular\SheetOptions;
use HyperTabular\ShortRows;
use HyperTabular\SuggestOptions;
use HyperTabular\SuggestStatus;
use HyperTabular\SuggestedColumn;
use HyperTabular\Suggestion;
use HyperTabular\TabularException;
use HyperTabular\TabularFailure;
use HyperTabular\Tests\Support\Failing;
use HyperTabular\Tests\Support\Package;
use HyperTabular\Workbook;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * A suggestion: what a sample of a source says of each column, a plan to inspect and hand on,
 * never applied. The corpus replays hold what it says of its cases; this holds the rest.
 */
final class SuggestTest extends TestCase
{
    private const NUMERIC = [
        Door::I8, Door::I16, Door::I32, Door::I64, Door::U8, Door::U16, Door::U32, Door::U64,
        Door::F32, Door::F64, Door::Decimal,
    ];

    private static function workbooks(): string
    {
        return \dirname(__DIR__, 2) . '/corpus/workbook';
    }

    private static function comma(): NumFormat
    {
        return new NumFormat(',', '.', NumFormat::ALL);
    }

    private static function indian(): NumFormat
    {
        return NumFormat::invariant()->withGrouping(NumFormat::GROUPING_INDIAN);
    }

    private static function suggest(string $data, ?SuggestOptions $options = null, ?Dialect $dialect = null): Suggestion
    {
        return DelimitedReader::suggestString($data, $dialect ?? Dialect::csv(), $options);
    }

    /** @return list<Door> */
    private static function doors(Suggestion $found): array
    {
        return array_map(static fn (SuggestedColumn $column): Door => $column->door, $found->report);
    }

    private static function rowsOf(int $count): string
    {
        return "a\n" . implode('', array_map(static fn (int $row): string => "{$row}\n", range(2, $count + 1)));
    }

    /** @param list<Column> $plan */
    private static function one(string $data, array $plan, ?Dialect $dialect = null): Batch
    {
        return DelimitedReader::fromString($data, $dialect ?? Dialect::csv(), $plan)->read();
    }

    private static function book(string $file): Workbook
    {
        return Workbook::open(self::workbooks() . '/' . $file);
    }

    /** @return array{Door, SuggestStatus} */
    private static function columnNamed(string $file, string $name): array
    {
        foreach (self::book($file)->sheet(0, new SheetOptions())->suggest()->report as $said) {
            if ($said->name === $name) {
                return [$said->door, $said->status];
            }
        }
        throw new \LogicException("{$file} has no column {$name}");
    }

    private function same(?NumFormat $found, NumFormat $wanted): void
    {
        $this->assertNotNull($found);
        $this->assertSame($wanted->packed(), $found->packed());
    }

    private static function temp(string $data): string
    {
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, $data);
        return $path;
    }

    public function testAReportSaysWhatThePlanDoesAndWhy(): void
    {
        $found = self::suggest(
            "flag,id,big,score,day,at,key,name,mixed,sep,order,none\n"
            . 'true,1,5000000000,2.5,2026-01-07,2026-01-07T00:00:00Z,6ba7b810-9dad-11d1-80b4-00c04fd430c8,'
            . "abc,1,\"1,234\",01/02/2026,\n"
            . 'false,2,6000000000,3,2026-01-08,2026-01-08T00:00:00Z,6ba7b811-9dad-11d1-80b4-00c04fd430c8,'
            . "def,x,\"2,345\",03/04/2026,\n"
        );
        $this->assertSame(
            [Door::Bool, Door::I32, Door::I64, Door::F64, Door::Date, Door::Timestamp, Door::Uuid, Door::Text,
                Door::Text, Door::I32, Door::DateOrdered, Door::Text],
            self::doors($found)
        );
        $this->assertSame(DateOrder::Mdy, $found->report[10]->declared);
        // Only what's resolved is read through a door: the last four read as text.
        $this->assertSame(
            [Door::Bool, Door::I32, Door::I64, Door::F64, Door::Date, Door::Timestamp, Door::Uuid, Door::Text,
                Door::Text, Door::Text, Door::Text, Door::Text],
            array_map(static fn (Column $column): Door => $column->door, $found->plan)
        );
        $this->assertSame(
            [...array_fill(0, 8, SuggestStatus::Resolved), SuggestStatus::Unresolved, SuggestStatus::Ambiguous,
                SuggestStatus::Ambiguous, SuggestStatus::Unresolved],
            array_map(static fn (SuggestedColumn $column): SuggestStatus => $column->status, $found->report)
        );
        foreach ($found->report as $ordinal => $said) {
            $this->assertSame([$ordinal, $ordinal], [$said->ordinal, $found->plan[$ordinal]->ordinal]);
        }
        $this->assertSame(['flag', 'none'], [$found->report[0]->name, $found->report[11]->name]);
        $this->assertSame([1, 2], [$found->report[8]->matched, $found->report[8]->sampled]);
        $this->assertSame([0, 0], [$found->report[11]->matched, $found->report[11]->sampled]);
        $this->assertSame([2, 2], [$found->report[9]->matched, $found->report[9]->sampled]);
        $this->assertSame([], $found->report[0]->candidates);
        $this->assertSame([DateOrder::Mdy, DateOrder::Dmy], $found->report[10]->candidates);
        [$first, $second] = $found->report[9]->candidates;
        $this->same($first, NumFormat::invariant());
        $this->assertSame([',', '.'], [$second->decimalSep, $second->groupSep]);
        $this->assertNull($found->report[0]->declared);
        $this->assertFalse($found->truncated);
    }

    public function testAPlanReadsBackWhatWasSampled(): void
    {
        $data = "flag,id,score,day,order,name\n1,2,2.5,2026-01-07,13/02/2026,abc\n0,3,3,2026-01-08,01/02/2026,def\n";
        $batch = self::one($data, self::suggest($data)->plan);
        $this->assertSame([2, []], [$batch->rows(), $batch->faults()]);
        $this->assertSame([true, false], $batch->values(0));
        $this->assertSame([2, 3], $batch->values(1));
        $this->assertSame([2.5, 3.0], $batch->values(2));
        $this->assertSame('def', $batch->values(5)[1]);
    }

    public function testADeclaredFormatReachesThePlan(): void
    {
        $data = "a,b\n\"1,234\",\"1,234\"\n";
        $comma = self::comma();
        $found = self::suggest($data, new SuggestOptions(format: NumFormat::invariant(), columnFormats: [1 => $comma]));
        $this->assertSame([Door::I32, Door::F64], self::doors($found));
        $this->assertSame(',', $found->plan[1]->format->decimalSep);
        // What the caller declared comes back as the caller's own object.
        $this->assertSame($comma, $found->report[1]->format);
        $this->assertSame($comma, $found->plan[1]->format);
        $batch = self::one($data, $found->plan);
        $this->assertSame([[1234], [1.234]], [$batch->values(0), $batch->values(1)]);
        // A column's own is ahead of everything's, and everything's is the rest's.
        $indian = self::indian();
        $found = self::suggest(
            "a,b\n\"1,23,456\",\"1,234,567\"\n",
            new SuggestOptions(format: $indian, columnFormats: [1 => NumFormat::invariant()])
        );
        $this->assertSame([Door::I32, Door::I32], self::doors($found));
        $this->assertSame($indian, $found->report[0]->format);
        $this->same($found->report[1]->format, NumFormat::invariant());
    }

    public function testOptionsAreCheckedWhereTheyAreMadeOrUsed(): void
    {
        foreach ([-1, PHP_INT_MIN] as $rows) {
            try {
                new SuggestOptions($rows);
                $this->fail("{$rows} rows taken");
            } catch (\InvalidArgumentException $refused) {
                $this->assertSame("A sample can't be negative; got {$rows}", $refused->getMessage());
            }
        }
        $this->assertSame(SuggestOptions::DEFAULT_SAMPLE_ROWS, (new SuggestOptions())->sampleRows);
        $this->assertSame(1000, SuggestOptions::DEFAULT_SAMPLE_ROWS);
        $refusals = [
            [
                new SuggestOptions(columnFormats: ['x' => NumFormat::invariant()]),
                "SuggestOptions' columnFormats is keyed by source column"
            ],
            [
                new SuggestOptions(columnFormats: [-1 => NumFormat::invariant()]),
                "SuggestOptions' columnFormats is keyed by source column"
            ],
            [
                new SuggestOptions(columnFormats: [0 => 'x']),
                "SuggestOptions' columnFormats[0] must be a HyperCast\\NumFormat"
            ],
            [
                new SuggestOptions(columnFormats: [0 => DateOrder::Dmy]),
                "SuggestOptions' columnFormats[0] must be a HyperCast\\NumFormat"
            ],
            [
                new SuggestOptions(columnDateOrders: ['x' => DateOrder::Dmy]),
                "SuggestOptions' columnDateOrders is keyed by source column"
            ],
            [new SuggestOptions(columnDateOrders: [0 => 'x']), 'A date order must be a HyperCast\\DateOrder'],
            [
                new SuggestOptions(columnDateOrders: [0 => \HyperCast\ExcelEpoch::Y1900]),
                'A date order must be a HyperCast\\DateOrder'
            ],
        ];
        $sheet = self::book('basic.xlsx')->sheet(0, new SheetOptions());
        foreach ($refusals as [$options, $why]) {
            $calls = [static fn () => self::suggest("a\n1\n", $options), static fn () => $sheet->suggest($options)];
            foreach ($calls as $call) {
                try {
                    $call();
                    $this->fail("{$why}: taken");
                } catch (\InvalidArgumentException $refused) {
                    $this->assertSame($why, $refused->getMessage());
                }
            }
        }
    }

    public function testSuggestAppliesNothing(): void
    {
        $data = "id,score\n1,2.5\n2,3.5\n";
        $path = self::temp($data);
        try {
            $this->assertSame([Door::I32, Door::F64], self::doors(DelimitedReader::suggest($path, Dialect::csv())));
            $declared = [Column::text(0), Column::text(1)];
            $held = DelimitedReader::open($path, Dialect::csv(), $declared);
            $this->assertSame($declared, $held->plan());
            $batch = $held->read();
            $this->assertSame(['1', '2.5'], [$batch->values(0)[0], $batch->values(1)[0]]);
            $held->close();
        } finally {
            unlink($path);
        }
        $unbound = DelimitedReader::fromString($data, Dialect::csv());
        self::suggest($data);
        $this->assertFalse($unbound->isBound());
        $this->assertSame([], $unbound->plan());
        $sheet = self::book('basic.xlsx')->sheet(0, new SheetOptions());
        $sheet->suggest();
        $this->assertFalse($sheet->isBound());
        $this->assertSame([], $sheet->plan());
        $this->expectException(\LogicException::class);
        $sheet->read();
    }

    public function testABoundOrPartReadSheetAnswersTheSameAndStaysWhereItIs(): void
    {
        $opened = self::book('libreoffice-data.xlsx');
        $options = new SheetOptions(batchRows: 1);
        $asked = new SuggestOptions(1);
        $fresh = $opened->sheet(0, $options)->suggest($asked);
        $plan = array_map(static fn (int $at): Column => Column::text($at), array_keys($fresh->plan));
        $rows = [];
        $sheet = $opened->sheet(0, $options, $plan);
        while (($batch = $sheet->read()) !== null) {
            $rows[] = $batch->values(0)[0];
        }
        $this->assertGreaterThan(1, \count($rows));

        $sheet = $opened->sheet(0, $options, $plan);
        $this->assertEquals($fresh, $sheet->suggest($asked));
        $this->assertSame($rows[0], $sheet->read()->values(0)[0]);
        $this->assertEquals($fresh, $sheet->suggest($asked));
        $this->assertSame($plan, $sheet->plan());
        $this->assertSame($rows[1], $sheet->read()->values(0)[0]);
    }

    public function testANamedSheetIsTheSheetSampled(): void
    {
        $opened = self::book('multisheet.xlsx');
        $answered = 0;
        foreach ($opened->sheets() as $at => $info) {
            try {
                $byIndex = $opened->sheet($at, new SheetOptions());
            } catch (TabularException) {
                try {
                    $opened->sheet($info->name, new SheetOptions());
                    $this->fail("{$info->name} opened by name");
                } catch (TabularException) {
                    continue;
                }
            }
            $this->assertEquals($byIndex->suggest(), $opened->sheet($info->name, new SheetOptions())->suggest());
            $answered++;
        }
        $this->assertGreaterThan(1, $answered);
    }

    /** @return iterable<string, array{int, bool}> */
    public static function exactSamples(): iterable
    {
        foreach ([2 => false, 3 => false, 4 => true, 5 => true, 9000 => true] as $rows => $truncated) {
            yield "{$rows} rows" => [$rows, $truncated];
        }
    }

    #[DataProvider('exactSamples')]
    public function testASampleOfExactlyTheSourceIsNotTruncated(int $rows, bool $truncated): void
    {
        $exact = new SuggestOptions(3);
        $found = self::suggest(self::rowsOf($rows), $exact);
        $this->assertSame($truncated, $found->truncated);
        $this->assertSame(min($rows, 3), $found->report[0]->sampled);
        $path = self::temp(self::rowsOf($rows));
        try {
            $this->assertSame($truncated, DelimitedReader::suggest($path, Dialect::csv(), $exact)->truncated);
        } finally {
            unlink($path);
        }
    }

    public function testASampleIsAThousandRowsAndAtMostABatch(): void
    {
        $data = self::rowsOf(9000);
        $default = self::suggest($data);
        $this->assertSame([1000, true], [$default->report[0]->sampled, $default->truncated]);
        $most = new SuggestOptions(1_000_000);
        $found = self::suggest($data, $most);
        $this->assertSame([4096, true], [$found->report[0]->sampled, $found->truncated]);
        $found = self::suggest(self::rowsOf(4096), $most);
        $this->assertSame([4096, false], [$found->report[0]->sampled, $found->truncated]);
    }

    /** Zero, omitted and an explicit default are three things, and each is what it says. */
    public function testASampleOfNoneIsOneAndADeclarationAloneLeavesTheThousand(): void
    {
        $data = "a\n1\n2.5\n";
        $found = self::suggest($data, new SuggestOptions(0));
        $this->assertSame([Door::Bool], self::doors($found));
        $this->assertSame([1, true], [$found->report[0]->sampled, $found->truncated]);
        $found = self::suggest($data, new SuggestOptions(sampleRows: 0));
        $this->assertSame([1, true], [$found->report[0]->sampled, $found->truncated]);
        $defaults = [
            null, new SuggestOptions(), new SuggestOptions(SuggestOptions::DEFAULT_SAMPLE_ROWS),
            new SuggestOptions(format: NumFormat::invariant()), new SuggestOptions(dateOrder: DateOrder::Dmy),
        ];
        foreach ($defaults as $options) {
            $found = self::suggest($data, $options);
            $this->assertSame([Door::F64], self::doors($found));
            $this->assertSame([2, false], [$found->report[0]->sampled, $found->truncated]);
        }
        $sheet = self::book('basic.xlsx')->sheet(0, new SheetOptions());
        $this->assertSame(1, $sheet->suggest(new SuggestOptions(0))->report[0]->sampled);
        $this->assertGreaterThan(
            1,
            $sheet->suggest(new SuggestOptions(format: NumFormat::invariant()))->report[0]->sampled
        );
        $this->assertGreaterThan(1, $sheet->suggest()->report[0]->sampled);
    }

    public function testWhatTheSampleDidNotReachHasNoSay(): void
    {
        $data = self::rowsOf(3) . "oops\n";
        $found = self::suggest($data, new SuggestOptions(2));
        $this->assertSame([Door::I32], self::doors($found));
        $this->assertSame([SuggestStatus::Resolved, true], [$found->report[0]->status, $found->truncated]);
        $this->assertSame(SuggestStatus::Unresolved, self::suggest($data)->report[0]->status);
    }

    public function testASheetsSampleIsAtMostItsBatch(): void
    {
        $found = self::book('libreoffice-data.xlsx')->sheet(0, new SheetOptions(batchRows: 1))->suggest();
        $this->assertTrue($found->truncated);
        foreach ($found->report as $said) {
            $this->assertLessThanOrEqual(1, $said->sampled);
        }
        $rows = implode(
            '',
            array_map(static fn (int $row): string => "<row r=\"{$row}\"><c><v>{$row}</v></c></row>", range(1, 8))
        );
        $opened = Workbook::fromString(Package::xlsx($rows));
        $found = $opened->sheet(0, new SheetOptions(false, batchRows: 5))->suggest();
        $this->assertSame([true, 5], [$found->truncated, $found->report[0]->sampled]);
        $whole = $opened->sheet(0, new SheetOptions(false))->suggest();
        $this->assertSame([false, 8], [$whole->truncated, $whole->report[0]->sampled]);
    }

    public function testAFailureAmongTheSampledRowsIsSaidAndOnePastThemIsNot(): void
    {
        $data = "a,b\n1,2\n3,4\n5\n7,8\n";
        try {
            self::suggest($data);
            $this->fail('a ragged sample suggested');
        } catch (TabularException $failure) {
            $this->assertSame(TabularFailure::ColumnCount, $failure->kind);
        }
        $found = self::suggest($data, new SuggestOptions(2));
        $this->assertSame([true, 2], [$found->truncated, $found->report[0]->sampled]);
        // Declared, a ragged row is a read like any other, and what a row hasn't got is blank.
        $padded = new Dialect(',', shortRows: ShortRows::Pad);
        $this->assertSame(3, self::suggest($data, null, $padded)->report[1]->sampled);
        $long = new Dialect(',', longRows: LongRows::Truncate);
        $this->assertCount(2, self::suggest("a,b\n1,2,3\n", null, $long)->plan);
        // A read's failure past the sample is truncation.
        $broken = self::rowsOf(3) . "\"broken\n";
        $found = self::suggest($broken, new SuggestOptions(2));
        $this->assertSame([true, 2], [$found->truncated, $found->report[0]->sampled]);
        $this->expectException(TabularException::class);
        self::suggest($broken);
    }

    public function testAColumnOfTokensIsAColumnOfBlanks(): void
    {
        $dialect = new Dialect(',', tokens: ['NA', '-']);
        $found = self::suggest("a,b,c\nNA,1,\"NA\"\n-,NA,\"NA\"\nNA,3,\"NA\"\n", null, $dialect);
        $this->assertSame(SuggestStatus::Unresolved, $found->report[0]->status);
        $this->assertSame(
            [0, 0, Door::Text],
            [$found->report[0]->matched, $found->report[0]->sampled, $found->plan[0]->door]
        );
        $this->assertSame([Door::I32, SuggestStatus::Resolved], [$found->report[1]->door, $found->report[1]->status]);
        $this->assertSame([2, 2], [$found->report[1]->matched, $found->report[1]->sampled]);
        // A quoted cell is never a token, so it's the text NA.
        $this->assertSame(
            [Door::Text, SuggestStatus::Resolved, 3],
            [$found->report[2]->door, $found->report[2]->status, $found->report[2]->sampled]
        );
    }

    public function testInvalidTextIsAFailedCellAndNotABlank(): void
    {
        $found = self::suggest("a,b\n\xE9t\xE9,1\nabc,\xFF\n");
        $this->assertSame(SuggestStatus::Unresolved, $found->report[0]->status);
        $this->assertSame([0, 2], [$found->report[0]->matched, $found->report[0]->sampled]);
        $this->assertSame([1, 2], [$found->report[1]->matched, $found->report[1]->sampled]);
    }

    public function testAQuotesDoubledTextIsWhatIsJudged(): void
    {
        $this->assertSame([Door::Text], self::doors(self::suggest("a\n\"1\"\"\"\n")));
        $this->assertSame([Door::I32], self::doors(self::suggest("a\n\"1\"\n\"2\"\n")));
    }

    public function testNoHeaderHasNoNamesAndTheFirstRecordsWidth(): void
    {
        $found = self::suggest("1,x\n2,y\n", null, new Dialect(',', hasHeader: false));
        $this->assertSame([Door::I32, Door::Text], self::doors($found));
        $this->assertSame([null, 2], [$found->report[0]->name, $found->report[0]->sampled]);
    }

    public function testNothingInTheSourceIsNothingToPlan(): void
    {
        foreach ([Dialect::csv(), new Dialect(',', hasHeader: false)] as $dialect) {
            $found = self::suggest('', null, $dialect);
            $this->assertSame([[], [], false], [$found->plan, $found->report, $found->truncated]);
        }
        $found = self::suggest("a,b\n");
        $this->assertSame([Door::Text, Door::Text], self::doors($found));
        $this->assertSame([SuggestStatus::Unresolved, 0], [$found->report[0]->status, $found->report[0]->sampled]);
    }

    public function testADialectsDeclarationsAreTheSamples(): void
    {
        $dialect = new Dialect("\t", skip: 1, headerRow: 1);
        $found = self::suggest("a title, with a comma\nignored\tcolumns\nid\tscore\n7\t2.5\n", null, $dialect);
        $this->assertSame([Door::I32, Door::F64], self::doors($found));
        $this->assertSame('score', $found->report[1]->name);
        // Latin-1, decoded before anything's judged, and a string and a file say the same.
        $latin = new Dialect(',', encoding: Encoding::Latin1);
        $data = "caf\xE9\n7\n8\n";
        $found = self::suggest($data, null, $latin);
        $this->assertSame(['café', Door::I32], [$found->report[0]->name, $found->report[0]->door]);
        $path = self::temp($data);
        try {
            $this->assertEquals($found, DelimitedReader::suggest($path, $latin));
        } finally {
            unlink($path);
        }
        // An encoding failure among the sample is the sample's.
        try {
            self::suggest("a\0\n\0\0\x001\0\n\0\0\xD8", null, new Dialect(',', encoding: Encoding::Utf16Le));
            $this->fail('a broken UTF-16 sample suggested');
        } catch (TabularException $failure) {
            $this->assertSame(TabularFailure::Encoding, $failure->kind);
        }
    }

    public function testAPathThatIsNotThereIsTheErrorOpeningItGives(): void
    {
        $this->expectException(\RuntimeException::class);
        $this->expectExceptionMessage('/no/such/file.csv');
        @DelimitedReader::suggest('/no/such/file.csv', Dialect::csv());
    }

    public function testASheetWithNoHeaderHasNoNamesAndItsFirstRowsWidth(): void
    {
        $found = self::book('basic.xlsx')->sheet(0, new SheetOptions(false))->suggest();
        $this->assertSame([9, null], [\count($found->plan), $found->report[0]->name]);
        // The header row is a row here: `id` and its four numbers, one door taking four of five.
        $this->assertSame(
            [Door::Text, 4, 5],
            [$found->report[0]->door, $found->report[0]->matched, $found->report[0]->sampled]
        );
    }

    public function testATypedCellIsJudgedByWhatTheTextDoorSaysOfIt(): void
    {
        $resolved = static fn (Door $door): array => [$door, SuggestStatus::Resolved];
        foreach (['basic.xlsx', 'libreoffice-data.xlsx', 'libreoffice-data.ods', 'numbers-data.numbers'] as $file) {
            $this->assertSame($resolved(Door::I32), self::columnNamed($file, 'id'), $file);
            $this->assertSame($resolved(Door::Bool), self::columnNamed($file, 'flag'), $file);
        }
        // A date, a date-time and a time of day, each as its writer's canonical text.
        foreach (['basic.xlsx', 'libreoffice-data.ods', 'numbers-data.numbers'] as $file) {
            foreach (['day', 'when', 'at'] as $name) {
                $this->assertSame($resolved(Door::Text), self::columnNamed($file, $name), "{$file}: {$name}");
            }
        }
        $this->assertSame($resolved(Door::Text), self::columnNamed('libreoffice-data.ods', 'elapsed'));
        $this->assertSame($resolved(Door::F64), self::columnNamed('basic.xlsx', 'amount'));
        $this->assertSame($resolved(Door::F64), self::columnNamed('numbers-data.numbers', 'pct'));
        $this->assertSame($resolved(Door::F64), self::columnNamed('libreoffice-data.ods', 'pct'));
        $this->assertSame($resolved(Door::I32), self::columnNamed('basic.ods', 'money'));
        // A column that mixes kinds says so, with a formula's cell an empty one.
        $this->assertSame([Door::Text, SuggestStatus::Unresolved], self::columnNamed('numbers-data.numbers', 'calc'));
        $this->assertSame([Door::Text, SuggestStatus::Unresolved], self::columnNamed('basic.ods', 'stamp'));
    }

    /** Every sheet of every corpus package, under nothing declared, Western, the comma decimal and Indian. */
    public function testNoSheetsPlanRefusesACellItSampled(): void
    {
        $planned = $typed = $refusedBySuggest = 0;
        $readFails = static function (Workbook $opened, int $at): bool {
            try {
                $sheet = $opened->sheet($at, new SheetOptions(), [Column::text(0)]);
                while ($sheet->read() !== null) {
                    // Read to the end.
                }
                return false;
            } catch (TabularException) {
                return true;
            }
        };
        $files = scandir(self::workbooks());
        sort($files);
        foreach ($files as $file) {
            $path = self::workbooks() . '/' . $file;
            if (str_ends_with($file, '.py') || !is_file($path)) {
                continue;
            }
            try {
                $opened = Workbook::open($path);
            } catch (TabularException) {
                continue;
            }
            foreach (array_keys($opened->sheets()) as $at) {
                foreach ([null, NumFormat::invariant(), self::comma(), self::indian()] as $declared) {
                    try {
                        $sheet = $opened->sheet($at, new SheetOptions());
                    } catch (TabularException) {
                        continue;
                    }
                    try {
                        $found = $sheet->suggest(new SuggestOptions(format: $declared));
                    } catch (TabularException $failure) {
                        $this->assertTrue($readFails($opened, $at), "{$file} {$at}: {$failure->getMessage()}");
                        $refusedBySuggest++;
                        continue;
                    }
                    if ($found->plan === []) {
                        continue;
                    }
                    $planned++;
                    $typed += \count(
                        array_filter($found->plan, static fn (Column $column): bool => $column->door !== Door::Text)
                    );
                    $rows = 0;
                    $sheet = $opened->sheet($at, new SheetOptions(), $found->plan);
                    while ($rows < 1000 && ($batch = $sheet->read()) !== null) {
                        foreach ($batch->faults() as $problem) {
                            $refused = $problem->row + $rows < 1000
                                && \in_array($problem->reason, [CastFailure::Malformed, CastFailure::OutOfRange], true)
                                && $found->plan[$problem->planColumn ?? 0]->door !== Door::Text;
                            $this->assertFalse($refused, "{$file} {$at}");
                        }
                        $rows += $batch->rows();
                    }
                }
            }
        }
        $this->assertGreaterThanOrEqual(80, $planned);
        $this->assertGreaterThanOrEqual(350, $typed);
        $this->assertGreaterThanOrEqual(1, $refusedBySuggest);
    }

    /** @return iterable<string, array{string, list<string>}> */
    public static function numberColumns(): iterable
    {
        yield 'basic.xlsx' => ['basic.xlsx', ['amount', 'ratio']];
        yield 'basic.ods' => ['basic.ods', ['amount', 'pct']];
        yield 'excel-web-data.xlsx' => ['excel-web-data.xlsx', ['pct', 'money']];
    }

    /** @param list<string> $names */
    #[DataProvider('numberColumns')]
    public function testAWorkbooksNumberColumnIsNumericUnderAnyFormat(string $file, array $names): void
    {
        foreach ([null, NumFormat::invariant(), self::comma()] as $asked) {
            $found = self::book($file)->sheet(0, new SheetOptions())->suggest(new SuggestOptions(format: $asked));
            foreach ($names as $name) {
                $said = array_values(
                    array_filter($found->report, static fn (SuggestedColumn $column): bool => $column->name === $name)
                )[0];
                $this->assertSame(SuggestStatus::Resolved, $said->status, "{$file} {$name}");
                $this->assertContains($said->door, self::NUMERIC, "{$file} {$name}");
            }
        }
    }

    /** @return iterable<string, array{string}> */
    public static function typedNumbers(): iterable
    {
        foreach (
            ['2147483647', '2147483648', '-2147483648', '-2147483649', '9223372036854775807', '-9223372036854775808',
                '9223372036854775808', '18446744073709551616', '1e21', '1.5E300', '1.7976931348623157E+308', '5e-324',
                '-0', '0.1', '1234.5', 'NaN', 'Infinity', '1e999'] as $value
        ) {
            yield $value => [$value];
        }
    }

    #[DataProvider('typedNumbers')]
    public function testATypedNumberAtADoorsLimitReadsThroughTheDoorItSuggests(string $value): void
    {
        $opened = Workbook::fromString(Package::xlsx("<row r=\"1\"><c><v>{$value}</v></c></row>"));
        $options = new SheetOptions(false);
        foreach ([null, NumFormat::invariant(), self::comma()] as $asked) {
            $found = $opened->sheet(0, $options)->suggest(new SuggestOptions(format: $asked));
            $batch = $opened->sheet(0, $options, $found->plan)->read();
            foreach ($batch->faults() as $problem) {
                $this->assertNotContains($problem->reason, [CastFailure::Malformed, CastFailure::OutOfRange], $value);
            }
            $this->assertNotSame(SuggestStatus::Ambiguous, $found->report[0]->status, $value);
        }
    }

    public function testASheetsSampleKeepsItsNotesWhateverAReadWasToHave(): void
    {
        $opened = self::book('suggest.xlsx');
        $found = $opened->sheet(0, new SheetOptions())->suggest();
        $this->assertSame(
            [SuggestStatus::Resolved, SuggestStatus::Ambiguous, SuggestStatus::Ambiguous],
            array_map(
                static fn (SuggestedColumn $column): SuggestStatus => $column->status,
                \array_slice($found->report, 0, 3)
            )
        );
        $this->assertEquals($found, $opened->sheet(0, new SheetOptions(notes: false))->suggest());
        // The owner's rule on a text cell in a sheet with notes off.
        $opened = Workbook::fromString(Package::xlsx('<row r="1"><c t="inlineStr"><is><t>1,234</t></is></c></row>'));
        $found = $opened->sheet(0, new SheetOptions(false, notes: false))->suggest();
        $this->assertSame(SuggestStatus::Ambiguous, $found->report[0]->status);
        $this->assertCount(2, $found->report[0]->candidates);
    }

    public function testADeclaredDateOrderSettlesADateTheSampleLeftOpen(): void
    {
        $data = "day\n01/02/2026\n03/04/2026\n";
        $found = self::suggest($data);
        $this->assertSame([SuggestStatus::Ambiguous, Door::Text], [$found->report[0]->status, $found->plan[0]->door]);
        $found = self::suggest($data, new SuggestOptions(dateOrder: DateOrder::Dmy));
        $this->assertSame([SuggestStatus::Resolved, []], [$found->report[0]->status, $found->report[0]->candidates]);
        $this->assertSame(
            [Door::DateOrdered, DateOrder::Dmy->value, DateOrder::Dmy],
            [$found->plan[0]->door, $found->plan[0]->declared, $found->report[0]->declared]
        );
        // A column's own wins over everything's.
        $found = self::suggest(
            $data,
            new SuggestOptions(dateOrder: DateOrder::Dmy, columnDateOrders: [0 => DateOrder::Mdy])
        );
        $this->assertSame(DateOrder::Mdy, $found->report[0]->declared);
        // An order the column's cells rule out doesn't overrule them.
        $found = self::suggest("day\n13/02/2026\n", new SuggestOptions(dateOrder: DateOrder::Mdy));
        $this->assertSame(DateOrder::Dmy, $found->report[0]->declared);
        // An order out of the question's set, as year-first is, leaves the question open.
        $found = self::suggest($data, new SuggestOptions(dateOrder: DateOrder::Ymd));
        $this->assertSame([SuggestStatus::Ambiguous, Door::Text], [$found->report[0]->status, $found->plan[0]->door]);
    }

    public function testACommaThatProvedToGroupStaysWesternInThePlan(): void
    {
        $data = "a\n\"1,234\"\n2.5\n\"1,23,456\"\n";
        $found = self::suggest($data, new SuggestOptions(2));
        $this->assertSame([Door::F64, SuggestStatus::Resolved], [$found->report[0]->door, $found->report[0]->status]);
        $this->same($found->report[0]->format, NumFormat::invariant());
        $this->same($found->plan[0]->format, NumFormat::invariant());
        $batch = self::one($data, $found->plan);
        $this->assertSame([1234.0, 2.5], \array_slice($batch->values(0), 0, 2));
        $this->assertSame(CastFailure::Malformed, $batch->get(0, 2)->reason);
        // A month-day-year date's parameter is the same 2, and says nothing of a notation.
        $found = self::suggest("a\n02/13/2026\n");
        $this->assertSame([DateOrder::Mdy, null], [$found->report[0]->declared, $found->report[0]->format]);
        // Declared, it's the declared format, and the report names that.
        $indian = self::indian();
        $found = self::suggest("a\n\"1,23,456\"\n", new SuggestOptions(format: $indian));
        $this->assertSame($indian, $found->report[0]->format);
        $this->same($found->plan[0]->format, $indian);
        $this->assertEquals(new Success(123_456), self::one("a\n\"1,23,456\"\n", $found->plan)->get(0, 0));
        // A column that isn't a resolved number names no format.
        $this->assertNull(self::suggest("a\nx\n")->report[0]->format);
        $found = self::suggest("a\n\"1,234\"\n");
        $this->assertSame([SuggestStatus::Ambiguous, null], [$found->report[0]->status, $found->report[0]->format]);
    }

    public function testANumberWithNoFormatDeclaredIsPlannedWestern(): void
    {
        $data = "a\n7\n8\n\"1,23,456\"\n";
        $found = self::suggest($data, new SuggestOptions(2));
        $this->assertSame([Door::I32, SuggestStatus::Resolved], [$found->report[0]->door, $found->report[0]->status]);
        $this->same($found->report[0]->format, NumFormat::invariant());
        $this->same($found->plan[0]->format, NumFormat::invariant());
        $batch = self::one($data, $found->plan);
        $this->assertSame([7, 8], \array_slice($batch->values(0), 0, 2));
        $this->assertSame(CastFailure::Malformed, $batch->get(0, 2)->reason);
        // An undeclared Indian shape is text, beside a number planned under flags 95.
        $found = self::suggest("a,b\n\"1,23,456\",7\n");
        $this->assertSame([Door::Text, SuggestStatus::Resolved], [$found->report[0]->door, $found->report[0]->status]);
        $this->same($found->plan[1]->format, NumFormat::invariant());
    }

    public function testAHeaderlessFirstRecordIsNotJudgedAsAHeader(): void
    {
        $headerless = new Dialect(',', hasHeader: false);
        $data = "\xE9t\xE9,1\nx,2\n";
        $found = self::suggest($data, null, $headerless);
        $this->assertSame([2, SuggestStatus::Unresolved], [\count($found->plan), $found->report[0]->status]);
        $this->assertSame(
            [0, 2, Door::I32],
            [$found->report[0]->matched, $found->report[0]->sampled, $found->report[1]->door]
        );
        $path = self::temp($data);
        try {
            $this->assertEquals($found, DelimitedReader::suggest($path, $headerless));
        } finally {
            unlink($path);
        }
        // With a header it is structural, and a read says the same.
        $calls = [
            static fn () => self::suggest($data),
            static fn () => DelimitedReader::fromString($data, Dialect::csv())
        ];
        foreach ($calls as $call) {
            try {
                $call();
                $this->fail('a header that is not UTF-8 was taken');
            } catch (TabularException $failure) {
                $this->assertSame(TabularFailure::Encoding, $failure->kind);
            }
        }
    }

    public function testASuggestionAndAReadAgreeOnALoneZero(): void
    {
        foreach ([["a\n\"0,125\"\n", NumFormat::invariant()], ["a\n0.125\n", self::comma()]] as [$data, $format]) {
            $found = self::suggest($data, new SuggestOptions(format: $format));
            $this->assertSame(
                [Door::Text, SuggestStatus::Resolved],
                [$found->report[0]->door, $found->report[0]->status]
            );
            $this->assertSame(CastFailure::Malformed, self::one($data, [Column::f64(0, $format)])->get(0, 0)->reason);
        }
        $found = self::suggest("a\n\"0,125\"\n");
        $this->same($found->report[0]->format, self::comma());
        $this->assertSame([0.125], self::one("a\n\"0,125\"\n", $found->plan)->values(0));
        // Beside `1,234`, it's what rules out the comma decimal.
        $found = self::suggest("a\n\"1,234\"\n0.125\n");
        $this->assertSame([Door::F64, SuggestStatus::Resolved], [$found->report[0]->door, $found->report[0]->status]);
        $this->same($found->report[0]->format, NumFormat::invariant());
        $found = self::suggest("a\n0.125\n");
        $this->same($found->report[0]->format, NumFormat::invariant());
        $this->assertSame([0.125], self::one("a\n0.125\n", $found->plan)->values(0));
    }

    public function testAColumnOnlyTheCommaDecimalReadsIsPlannedInIt(): void
    {
        $data = "f,i\n\"2,5\",1.234.567\n\"1.234,5\",1.000\n\"1,234\",\"1,234,567\"\n";
        $found = self::suggest($data, new SuggestOptions(2));
        $this->assertSame([Door::F64, Door::I32], self::doors($found));
        foreach ($found->report as $said) {
            $this->assertSame([SuggestStatus::Resolved, []], [$said->status, $said->candidates]);
            $this->same($said->format, self::comma());
        }
        $this->same($found->plan[0]->format, self::comma());
        $this->same($found->plan[1]->format, self::comma());
        $batch = self::one($data, $found->plan);
        $this->assertSame([2.5, 1234.5, 1.234], $batch->values(0));
        $this->assertSame([1_234_567, 1000], \array_slice($batch->values(1), 0, 2));
        $this->assertSame(CastFailure::Malformed, $batch->get(1, 2)->reason);
    }

    public function testAQuestionReadsAsTextAndEachAnswerReadsItsOwnValue(): void
    {
        $data = "a\n1.234\n";
        $found = self::suggest($data);
        $this->assertSame(
            [SuggestStatus::Ambiguous, null, Door::Text],
            [$found->report[0]->status, $found->report[0]->format, $found->plan[0]->door]
        );
        $read = array_map(
            static fn (NumFormat $format) => self::one($data, [Column::f64(0, $format)])->values(0)[0],
            $found->report[0]->candidates
        );
        $this->assertSame([1.234, 1234.0], $read);
    }

    public function testAPlanIsWhatAStreamReads(): void
    {
        $data = "id,score\n1,2.5\n2,3.5\n";
        $found = self::suggest($data);
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, $data);
        rewind($stream);
        $streamed = DelimitedReader::fromStream($stream, Dialect::csv(), $found->plan);
        $this->assertSame($found->plan, $streamed->plan());
        $this->assertSame([2.5, 3.5], $streamed->read()->values(1));
        fclose($stream);
    }

    /**
     * A path is opened as PHP opens it, wrappers included. A stream whose `fread()` fails (returns
     * `false`) is the source's failure: past a whole sample it only says the source was truncated,
     * and with the sample still short it is thrown. What the stream's own PHP throws is the
     * caller's, and is thrown wherever it comes, an `\ErrorException` a framework made of a warning
     * included.
     */
    public function testAFailedReadPastAWholeSampleIsTruncationAndAThrowIsTheCallers(): void
    {
        Failing::register();
        $asked = new SuggestOptions(2);
        Failing::$parts = ["a\n1\n2\n", false];
        $found = @DelimitedReader::suggest('hypertabular-failing://csv', Dialect::csv(), $asked);
        $this->assertSame([true, 2], [$found->truncated, $found->report[0]->sampled]);
        $thrown = [
            [new \RuntimeException('the wire went'), \RuntimeException::class, 'the wire went'],
            [new \LogicException('custom'), \LogicException::class, 'custom'],
            [new \ErrorException('fread(): read failed'), \ErrorException::class, 'fread(): read failed'],
        ];
        $short = [[false, \RuntimeException::class, 'hypertabular: reading the stream failed']];
        foreach ([["a\n1\n2\n", $thrown], ["a\n1\n", [...$thrown, ...$short]]] as [$text, $failures]) {
            foreach ($failures as [$failure, $class, $message]) {
                Failing::$parts = [$text, $failure];
                try {
                    @DelimitedReader::suggest('hypertabular-failing://csv', Dialect::csv(), $asked);
                    $this->fail("{$message}: suggested");
                } catch (\Throwable $caught) {
                    $this->assertInstanceOf($class, $caught);
                    $this->assertSame($message, $caught->getMessage());
                    if ($failure instanceof \Throwable) {
                        $this->assertSame($failure, $caught);
                    }
                }
            }
        }
    }

    public function testARangesUnresolvedIdColumnStaysUnresolved(): void
    {
        $ranged = new SheetOptions(range: Range::parse('B4:C12'));
        foreach (self::book('ranges.xlsx')->sheet(0, $ranged)->suggest()->report as $said) {
            if ($said->name === 'id') {
                $this->assertSame(
                    [Door::Text, SuggestStatus::Unresolved, 6, 7],
                    [$said->door, $said->status, $said->matched, $said->sampled]
                );
                return;
            }
        }
        $this->fail('no id column');
    }

    /**
     * No header, ragged rows and row widths kept: a suggestion samples the same however a read of
     * the source is batched, which here is the one-batch sample against reads of one to four rows.
     */
    public function testAHeaderlessRaggedSampleIsTheSameAcrossEachBatchBoundary(): void
    {
        $ragged = "1,x\n2,y,long\n3\n4,z\n5,w\n";
        $headerless = new Dialect(',', hasHeader: false, shortRows: ShortRows::Pad, longRows: LongRows::Report);
        for ($limit = 1; $limit <= 6; $limit++) {
            $whole = self::suggest($ragged, new SuggestOptions($limit), $headerless);
            $this->assertSame(min($limit, 5), $whole->report[0]->sampled, "sample {$limit}");
            $this->assertSame($limit < 5, $whole->truncated, "sample {$limit}");
            $this->assertCount(2, $whole->plan);
            // What a read sees of the rows sampled, in any batches, agrees with what was said.
            for ($rows = 1; $rows <= 4; $rows++) {
                $reader = DelimitedReader::fromString($ragged, $headerless, [Column::text(0), Column::text(1)], $rows);
                $seen = [];
                while (($batch = $reader->read()) !== null) {
                    foreach ($batch as $row) {
                        $seen[] = [$row->value(0), $row->value(1)];
                    }
                }
                $this->assertSame([['1', 'x'], ['2', 'y'], ['3', null], ['4', 'z'], ['5', 'w']], $seen);
            }
        }
        $this->assertSame([Door::I32, Door::Text], self::doors(self::suggest($ragged, null, $headerless)));
    }
}
