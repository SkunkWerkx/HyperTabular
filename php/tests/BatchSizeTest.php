<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\LongRows;
use HyperTabular\SheetOptions;
use HyperTabular\SuggestOptions;
use HyperTabular\Workbook;
use PHPUnit\Framework\Attributes\Group;
use PHPUnit\Framework\TestCase;

/**
 * A batch size is the caller's, and the core judges it before anything is allocated: a zero, a
 * size past the core's limit and a row past it are PHP's argument error in the core's words. A
 * negative count is no count at all, refused here before the core is asked.
 *
 * The `big-sizes` group can't hurt a host however right the code is, but it is what would if the core's
 * check went: the refusals sit just over the core's limit, so the arrays asked for would be a little
 * over 4 GiB, and the read at 1 << 21 rows holds about 125 MiB untouched. Run it where a mistake is
 * contained (a container capped at 4 GiB) when the code under it changes.
 */
final class BatchSizeTest extends TestCase
{
    private const OVER_LIMIT = '/\Abatch_rows asks for \d+ bytes, past the limit of 4294967296\z/';

    private static function book(): Workbook
    {
        return Workbook::open(\dirname(__DIR__, 2) . '/corpus/workbook/basic-1904.xlsx');
    }

    /** @return resource */
    private static function stream(string $bytes = "a\n1\n")
    {
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, $bytes);
        rewind($stream);
        return $stream;
    }

    /** `$call` throws PHP's argument error saying exactly `$message`, or matching it as a pattern. */
    private function refuses(string $message, callable $call): void
    {
        try {
            $call();
            $this->fail("taken, where it should say: {$message}");
        } catch (\InvalidArgumentException $refused) {
            if (str_starts_with($message, '/')) {
                $this->assertMatchesRegularExpression($message, $refused->getMessage());
            } else {
                $this->assertSame($message, $refused->getMessage());
            }
        }
    }

    /** Every entry point opened with a zero, a missing path's among them, and each omitted. */
    public function testAZeroIsRefusedForEachOptionWhereAnOmittedSizeHasItsDefault(): void
    {
        $plan = [Column::i32(0)];
        $text = "a\n1\n";
        $noRows = 'batch_rows must be at least 1';
        $noBuffer = 'buffer_bytes must be at least 1';
        $this->refuses($noRows, static fn () => DelimitedReader::fromString($text, Dialect::csv(), $plan, 0));
        $this->refuses($noRows, static fn () => DelimitedReader::fromString($text, Dialect::csv(), null, 0));
        $this->refuses(
            $noBuffer,
            static fn () => DelimitedReader::fromString($text, Dialect::csv(), $plan, bufferBytes: 0)
        );
        $this->refuses($noRows, static fn () => DelimitedReader::fromStream(self::stream(), Dialect::csv(), $plan, 0));
        $this->refuses($noRows, static fn () => DelimitedReader::fromStream(self::stream(), Dialect::csv(), null, 0));
        $this->refuses(
            $noBuffer,
            static fn () => DelimitedReader::fromStream(self::stream(), Dialect::csv(), bufferBytes: 0)
        );
        $this->refuses(
            $noBuffer,
            static fn () => DelimitedReader::fromStream(self::stream(), Dialect::csv(), $plan, bufferBytes: 0)
        );
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, $text);
        try {
            $this->refuses($noRows, static fn () => DelimitedReader::open($path, Dialect::csv(), $plan, 0));
            $this->refuses(
                $noBuffer,
                static fn () => DelimitedReader::open($path, Dialect::csv(), $plan, bufferBytes: 0)
            );
            $this->assertSame([1], DelimitedReader::open($path, Dialect::csv(), $plan)->read()->values(0));
        } finally {
            unlink($path);
        }
        $none = new SheetOptions(batchRows: 0);
        $this->assertSame(0, $none->batchRows);
        $this->refuses($noRows, static fn () => self::book()->sheet(0, $none, $plan));
        $this->refuses($noRows, static fn () => self::book()->sheet(0, $none));
        $this->refuses($noRows, static fn () => self::book()->sheet(99, $none));

        // Omitted, each is its default, and reads.
        $this->assertSame(DelimitedReader::DEFAULT_BATCH_ROWS, (new SheetOptions())->batchRows);
        $this->assertSame(4096, DelimitedReader::DEFAULT_BATCH_ROWS);
        $this->assertSame([1], DelimitedReader::fromString($text, Dialect::csv(), $plan)->read()->values(0));
        $this->assertSame([1], DelimitedReader::fromStream(self::stream(), Dialect::csv(), $plan)->read()->values(0));
        $this->assertGreaterThan(0, self::book()->sheet(0, new SheetOptions(), $plan)->read()->rows());

        // A sample of none is a sample of one; omitted, it is the thousand.
        $three = "a\n1\n2\n3\n";
        $this->assertSame(
            1,
            DelimitedReader::suggestString($three, Dialect::csv(), new SuggestOptions(0))->report[0]->sampled
        );
        $this->assertSame(3, DelimitedReader::suggestString($three, Dialect::csv())->report[0]->sampled);
        $this->assertSame(
            3,
            DelimitedReader::suggestString($three, Dialect::csv(), new SuggestOptions())->report[0]->sampled
        );
    }

    /** A path's sizes are judged before the file is touched. */
    public function testAPathsSizesAreJudgedBeforeTheFileIsTouched(): void
    {
        $nowhere = '/no/such/file.csv';
        try {
            @DelimitedReader::open($nowhere, Dialect::csv());
            $this->fail('a missing file opened');
        } catch (\RuntimeException $missing) {
            $this->assertNotInstanceOf(\InvalidArgumentException::class, $missing);
            $this->assertStringContainsString($nowhere, $missing->getMessage());
        }
        $this->refuses(
            'batch_rows must be at least 1',
            static fn () => DelimitedReader::open($nowhere, Dialect::csv(), null, 0)
        );
        $this->refuses(
            'buffer_bytes must be at least 1',
            static fn () => DelimitedReader::open($nowhere, Dialect::csv(), bufferBytes: 0)
        );
    }

    /** A negative count is no count, and refused here, in this binding's words; a zero is the core's. */
    public function testANegativeCountIsRefusedBeforeTheCoreIsAsked(): void
    {
        $this->refuses(
            'A batch must hold at least one row; got -1',
            static fn () => DelimitedReader::fromString("a\n", Dialect::csv(), null, -1)
        );
        $this->refuses(
            'The buffer must hold at least one byte; got -1',
            static fn () => DelimitedReader::fromString("a\n", Dialect::csv(), bufferBytes: -1)
        );
        $this->refuses('A batch must hold at least one row; got -1', static fn () => new SheetOptions(batchRows: -1));
        $this->refuses("A sample can't be negative; got -1", static fn () => new SuggestOptions(-1));
        $this->expectException(\TypeError::class);
        DelimitedReader::fromString("a\n", Dialect::csv(), null, '1');
    }

    #[Group('big-sizes')]
    public function testAPlanOfNoColumnsReadsAtAnyBatchSize(): void
    {
        $reader = DelimitedReader::fromString("a\n1\n", Dialect::csv(), [], 1 << 62);
        $this->assertSame(1, $reader->read()->rows());
        $this->assertNull($reader->read());
        $reader = DelimitedReader::fromString("a\n1\n", Dialect::csv(), [], PHP_INT_MAX);
        $this->assertSame(1, $reader->read()->rows());
    }

    public function testAModestBatchReadsTheSameAsARowAtATime(): void
    {
        $plan = [Column::text(0)];
        $one = self::book()->sheet(0, new SheetOptions(batchRows: 1), $plan);
        $values = [];
        while (($batch = $one->read()) !== null) {
            $values[] = $batch->values(0)[0];
        }
        $this->assertSame(
            $values,
            self::book()->sheet(0, new SheetOptions(batchRows: 1 << 12), $plan)->read()->values(0)
        );
    }

    #[Group('big-sizes')]
    public function testABatchPastTheCoresLimitIsRefusedByNameBeforeAllocating(): void
    {
        $plan = [Column::i32(0)];
        $justOver = 'batch_rows asks for 5377130496 bytes, past the limit of 4294967296';
        $this->refuses(
            $justOver,
            static fn () => DelimitedReader::fromString("a\n1\n", Dialect::csv(), $plan, 1 << 28)
        );
        $this->refuses(
            $justOver,
            static fn () => DelimitedReader::fromStream(self::stream(), Dialect::csv(), $plan, 1 << 28)
        );
        $this->refuses(
            $justOver,
            static fn () => DelimitedReader::open('/no/such/file.csv', Dialect::csv(), $plan, 1 << 28)
        );
        $this->refuses(
            'batch_rows asks for 4831838224 bytes, past the limit of 4294967296',
            static fn () => self::book()->sheet(0, new SheetOptions(batchRows: 1 << 27), $plan)
        );
        $this->refuses(
            self::OVER_LIMIT,
            static fn () => DelimitedReader::fromString("a\n1\n", Dialect::csv(), $plan, PHP_INT_MAX)
        );
    }

    #[Group('big-sizes')]
    public function testALatePlanPastTheLimitIsRefusedAndLeavesTheReaderAndTheSheetUnbound(): void
    {
        $plan = [Column::i32(0)];
        $reader = DelimitedReader::fromString("a\n1\n", Dialect::csv(), null, 1 << 28);
        $this->refuses(self::OVER_LIMIT, static fn () => $reader->bind($plan));
        $this->assertFalse($reader->isBound());
        $sheet = self::book()->sheet(0, new SheetOptions(batchRows: 1 << 27));
        $this->refuses(self::OVER_LIMIT, static fn () => $sheet->bind($plan));
        $this->assertFalse($sheet->isBound());
    }

    #[Group('big-sizes')]
    public function testAPlanOfNoColumnsIsRefusedWhereARowsWidthOrASheetsCellsAreKept(): void
    {
        $reporting = new Dialect(',', longRows: LongRows::Report);
        $this->refuses(
            self::OVER_LIMIT,
            static fn () => DelimitedReader::fromString("a\n1\n", $reporting, [], 1 << 31)
        );
        $this->refuses(
            self::OVER_LIMIT,
            static fn () => self::book()->sheet(0, new SheetOptions(batchRows: 1 << 30), [])
        );
    }

    /** A suggestion is never refused for the size of its own batch. */
    #[Group('big-sizes')]
    public function testASuggestionIsNotRefusedForTheSizeOfItsOwnBatch(): void
    {
        $width = 268_000;
        $names = implode(',', array_map(static fn (int $column): string => "c{$column}", range(0, $width - 1)));
        $row = implode(',', array_fill(0, $width, '1'));
        $wide = DelimitedReader::suggestString("{$names}\n{$row}\n{$row}\n", Dialect::csv());
        $this->assertCount($width, $wide->report);
        $this->assertSame(2, $wide->report[$width - 1]->sampled);
        $this->assertFalse($wide->truncated);
        unset($wide);
        // At exactly the limit with no plan, and a sample past every batch.
        $large = self::book()->sheet(0, new SheetOptions(batchRows: 1 << 29))->suggest(new SuggestOptions(1 << 40));
        $modest = self::book()->sheet(0, new SheetOptions())->suggest(new SuggestOptions(4096));
        $this->assertEquals($modest, $large);
    }

    /** A large batch's memory isn't written to; a read goes through it. */
    #[Group('big-sizes')]
    public function testALargeBatchReadsThroughWhatItAsksFor(): void
    {
        $rows = 1 << 21;
        $reader = DelimitedReader::fromString("a\n1\n", Dialect::csv(), [Column::i32(0)], $rows);
        $this->assertSame([1], $reader->read()->values(0));
        $sheet = self::book()->sheet(0, new SheetOptions(batchRows: $rows), [Column::text(0)]);
        $this->assertGreaterThan(0, $sheet->read()->rows());
    }
}
