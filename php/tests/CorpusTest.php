<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use DateTimeImmutable;
use DateTimeZone;
use HyperCast\CastFailure;
use HyperCast\DateOrder;
use HyperCast\Decimal;
use HyperCast\Duration;
use HyperCast\ExcelEpoch;
use HyperCast\Fault;
use HyperCast\NumFormat;
use HyperCast\Success;
use HyperCast\UnixPrecision;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Door;
use HyperTabular\TabularException;
use HyperTabular\TabularFailure;
use HyperTabular\Scratch;
use HyperTabular\SheetOptions;
use HyperTabular\Workbook;
use HyperTabular\WorkbookFormat;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * Replays the shared conformance corpus (corpus/delimited.json at the repository root) —
 * the same file the Rust and C# bindings replay — through this binding: from a string
 * handed over whole, from a string and a stream fed through buffers too small for a
 * record, and from a file, in batches of one row, of two, and of many. How the input is
 * cut up is the binding's business and must not change the answer.
 *
 * And corpus/workbook.json, the same way: each package opened from a string and from its
 * path, each sheet read by index and (where the name finds it) by name, in batches of one
 * row, of two, and of more than any sheet has.
 *
 * Decoded with JSON_BIGINT_AS_STRING so a u64 beyond PHP's signed int survives as digits.
 */
final class CorpusTest extends TestCase
{
    private const BATCH_ROWS = [1, 2, 1024];
    private const BUFFER_BYTES = [1, 5, 64, DelimitedReader::DEFAULT_BUFFER_BYTES];

    private const REASONS = [
        'empty' => CastFailure::Empty,
        'malformed' => CastFailure::Malformed,
        'out_of_range' => CastFailure::OutOfRange,
    ];

    /** The corpus directory at the repository root. */
    private static function directory(): string
    {
        $dir = __DIR__;
        // Stop when dirname() stops moving, not at '/': a Windows root is 'C:\\', never '/'.
        for ($parent = \dirname($dir); $parent !== $dir; $dir = $parent, $parent = \dirname($dir)) {
            if (is_file($dir . '/corpus/delimited.json')) {
                return $dir . '/corpus';
            }
        }
        throw new \RuntimeException('corpus/delimited.json not found above ' . __DIR__);
    }

    /** @return list<array<string, mixed>> */
    private static function corpus(string $file = 'delimited.json'): array
    {
        return json_decode(
            file_get_contents(self::directory() . '/' . $file),
            true,
            512,
            JSON_THROW_ON_ERROR | JSON_BIGINT_AS_STRING
        );
    }

    /** @return iterable<string, array{array<string, mixed>}> */
    public static function cases(): iterable
    {
        foreach (self::corpus() as $case) {
            yield $case['name'] => [$case];
        }
    }

    public function testTheCorpusIsTheWholeContract(): void
    {
        $corpus = self::corpus();
        $this->assertGreaterThanOrEqual(30, \count($corpus));
        // Every door the core has is exercised by some case.
        $doors = [];
        foreach ($corpus as $case) {
            foreach ($case['plan'] as $entry) {
                $doors[self::columnOf($entry)->door->name] = true;
            }
        }
        $this->assertCount(\count(Door::cases()), $doors);
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStringHandedOverWhole(array $case): void
    {
        foreach (self::BATCH_ROWS as $batchRows) {
            $this->replay(
                "{$case['name']} (string, {$batchRows} rows a batch)",
                $case,
                static fn (Dialect $dialect, array $plan): DelimitedReader =>
                    DelimitedReader::fromString($case['input'], $dialect, $plan, $batchRows)
            );
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStringFedThroughTinyBuffers(array $case): void
    {
        foreach (self::BATCH_ROWS as $batchRows) {
            foreach ([1, 5, 64] as $bufferBytes) {
                $this->replay(
                    "{$case['name']} (string through {$bufferBytes} bytes, {$batchRows} rows a batch)",
                    $case,
                    static fn (Dialect $dialect, array $plan): DelimitedReader =>
                        DelimitedReader::fromString($case['input'], $dialect, $plan, $batchRows, $bufferBytes)
                );
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStreamReadThroughBuffersOfEverySize(array $case): void
    {
        foreach (self::BATCH_ROWS as $batchRows) {
            foreach (self::BUFFER_BYTES as $bufferBytes) {
                $stream = fopen('php://memory', 'w+b');
                fwrite($stream, $case['input']);
                rewind($stream);
                try {
                    $this->replay(
                        "{$case['name']} (stream through {$bufferBytes} bytes, {$batchRows} rows a batch)",
                        $case,
                        static fn (Dialect $dialect, array $plan): DelimitedReader =>
                            DelimitedReader::fromStream($stream, $dialect, $plan, $batchRows, $bufferBytes)
                    );
                    // The stream is the caller's: the reader read it to its end and left it open.
                    $this->assertIsResource($stream);
                } finally {
                    fclose($stream);
                }
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAFile(array $case): void
    {
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, $case['input']);
        try {
            foreach ([2, 1024] as $batchRows) {
                foreach ([7, DelimitedReader::DEFAULT_BUFFER_BYTES] as $bufferBytes) {
                    $this->replay(
                        "{$case['name']} (file through {$bufferBytes} bytes, {$batchRows} rows a batch)",
                        $case,
                        static fn (Dialect $dialect, array $plan): DelimitedReader =>
                            DelimitedReader::open($path, $dialect, $plan, $batchRows, $bufferBytes)
                    );
                }
            }
        } finally {
            unlink($path);
        }
    }

    /** @return iterable<string, array{array<string, mixed>}> */
    public static function workbookCases(): iterable
    {
        foreach (self::corpus('workbook.json') as $case) {
            yield $case['name'] => [$case];
        }
    }

    public function testTheWorkbookCorpusIsTheWholeContract(): void
    {
        $corpus = self::corpus('workbook.json');
        $this->assertGreaterThanOrEqual(80, \count($corpus));
        $cells = 0;
        foreach ($corpus as $case) {
            if (isset($case['sheet'])) {
                $cells += \count($case['rows']) * \count($case['plan']);
            }
        }
        $this->assertGreaterThanOrEqual(12_000, $cells);
    }

    /**
     * The workbook corpus again with every buffer starting at one element and never asked to
     * be large up front: each read stops wherever the core runs out of room and resumes in a
     * grown buffer that has to have kept what the old one held. A grow that dropped it would
     * read garbage, and the corpus would say so.
     */
    public function testEveryWorkbookReadsTheSameWhenItsBuffersStartWithNoRoom(): void
    {
        Scratch::$stingy = true;
        Scratch::$grown = [0, 0, 0];
        try {
            foreach (self::corpus('workbook.json') as $case) {
                $this->testAWorkbook($case);
            }
        } finally {
            Scratch::$stingy = false;
        }
        // Not once per call: many times, mid-part, for each of the three.
        foreach (Scratch::$grown as $times) {
            $this->assertGreaterThan(1_000, $times, json_encode(Scratch::$grown));
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('workbookCases')]
    public function testAWorkbook(array $case): void
    {
        $path = self::directory() . '/' . $case['file'];
        $openings = [
            'string' => static fn (): Workbook => Workbook::fromString(file_get_contents($path)),
            'path' => static fn (): Workbook => Workbook::open($path),
        ];
        // A package the core refuses: every way of opening it gives the one failure.
        if (!isset($case['sheet'])) {
            foreach ($openings as $source => $open) {
                $failure = null;
                try {
                    $open();
                    $this->fail("{$case['name']} ({$source}): opened");
                } catch (TabularException $thrown) {
                    $failure = $thrown;
                }
                $this->assertFailure("{$case['name']} ({$source})", $case['failure'], $failure);
            }
            return;
        }

        $plan = array_map(self::columnOf(...), $case['plan']);
        $index = $case['sheet'];
        $name = $case['sheets'][$index]['name'];
        foreach ($openings as $source => $open) {
            $book = $open();
            $this->assertSame(WorkbookFormat::from($case['format'] === 'xlsx' ? 1 : 2), $book->format());
            $this->assertSame(ExcelEpoch::from($case['epoch']), $book->dateSystem());
            $this->assertSame(
                array_map(static fn (array $sheet): array => [$sheet['name'], $sheet['hidden']], $case['sheets']),
                array_map(static fn ($sheet): array => [$sheet->name, $sheet->hidden], $book->sheets())
            );
            $byName = array_search($name, array_map(static fn ($sheet): string => $sheet->name, $book->sheets()), true)
                === $index;
            foreach (self::BATCH_ROWS as $batchRows) {
                $options = new SheetOptions(
                    $case['options']['has_header'],
                    $case['options']['skip_empty_rows'],
                    $batchRows
                );
                foreach ($byName ? [$index, $name] : [$index] as $which) {
                    $label = "{$case['name']}: {$source}, {$batchRows} rows a batch, sheet "
                        . var_export($which, true);
                    $sheet = $book->sheet($which, $options, $plan);
                    $this->assertSame($case['header'], $sheet->header(), "{$label}: header");
                    $this->assertSame($plan, $sheet->plan(), $label);
                    $seen = 0;
                    $failure = null;
                    try {
                        while (($batch = $sheet->read()) !== null) {
                            $this->assertLessThanOrEqual($batchRows, $batch->rows(), $label);
                            for ($row = 0; $row < $batch->rows(); $row++) {
                                $this->assertSame($case['numbers'][$seen + $row], $batch->line($row), "{$label}, row");
                            }
                            $this->assertBatch($label, $batch, $plan, $case['rows'], $seen);
                            $seen += $batch->rows();
                        }
                    } catch (TabularException $thrown) {
                        $failure = $thrown;
                        // A failed sheet stays failed: the same failure, again.
                        try {
                            $sheet->read();
                            $this->fail("{$label}: a read after a structural failure succeeded");
                        } catch (TabularException $again) {
                            $this->assertSame($thrown, $again, $label);
                        }
                    }
                    $this->assertSame(\count($case['rows']), $seen, "{$label}: rows delivered");
                    $this->assertFailure($label, $case['failure'] ?? null, $failure);
                }
            }
        }
    }

    /**
     * @param array<string, mixed> $entry a plan entry of the corpus
     */
    private static function columnOf(array $entry): Column
    {
        $ordinal = $entry['ordinal'];
        $format = isset($entry['format'])
            ? new NumFormat(
                $entry['format']['decimal_sep'],
                $entry['format']['group_sep'],
                $entry['format']['flags'],
                $entry['format']['currency'] ?? ''
            )
            : null;
        return match ($entry['door']) {
            'bool' => Column::bool($ordinal),
            'i8' => Column::i8($ordinal, $format),
            'i16' => Column::i16($ordinal, $format),
            'i32' => Column::i32($ordinal, $format),
            'i64' => Column::i64($ordinal, $format),
            'u8' => Column::u8($ordinal, $format),
            'u16' => Column::u16($ordinal, $format),
            'u32' => Column::u32($ordinal, $format),
            'u64' => Column::u64($ordinal, $format),
            'f32' => Column::f32($ordinal, $format),
            'f64' => Column::f64($ordinal, $format),
            'decimal' => Column::decimal($ordinal, $format),
            'uuid' => Column::uuid($ordinal),
            'timestamp' => Column::timestamp($ordinal),
            'unix' => Column::unix($ordinal, UnixPrecision::from($entry['precision'])),
            'excel_serial' => Column::excelSerial($ordinal, ExcelEpoch::from($entry['epoch'])),
            'date' => Column::date($ordinal),
            'date_ordered' => Column::dateOrdered($ordinal, DateOrder::from($entry['order'])),
            'datetime' => Column::datetime($ordinal, DateOrder::from($entry['order'])),
            'time' => Column::time($ordinal),
            'duration' => Column::duration($ordinal),
            'text' => Column::text($ordinal),
        };
    }

    /**
     * @param array<string, mixed> $case
     * @param callable(Dialect, list<Column>): DelimitedReader $open
     */
    private function replay(string $label, array $case, callable $open): void
    {
        $dialect = new Dialect(
            $case['dialect']['separator'],
            $case['dialect']['quoting'],
            $case['dialect']['has_header'],
            $case['dialect']['skip_blank_lines']
        );
        $plan = array_map(self::columnOf(...), $case['plan']);
        $expected = $case['rows'];

        $reader = $open($dialect, $plan);
        $this->assertSame($case['header'], $reader->header(), "{$label}: header");
        $this->assertSame($plan, $reader->plan(), $label);

        $seen = 0;
        $failure = null;
        try {
            while (($batch = $reader->read()) !== null) {
                $this->assertBatch($label, $batch, $plan, $expected, $seen);
                $seen += $batch->rows();
            }
        } catch (TabularException $thrown) {
            $failure = $thrown;
            // A structural failure is final: the same one, again.
            try {
                $reader->read();
                $this->fail("{$label}: a read after a structural failure succeeded");
            } catch (TabularException $again) {
                $this->assertSame($thrown, $again, $label);
            }
        }
        $this->assertSame(\count($expected), $seen, "{$label}: rows delivered");
        $this->assertFailure($label, $case['failure'] ?? null, $failure);
        $reader->close();
    }

    /**
     * Holds one batch to the corpus's rows from `$seen` on, column by column: the column as a
     * whole, then each cell of it, the same answer both ways.
     *
     * @param list<Column> $plan the plan the batch was read through
     * @param list<list<array<string, mixed>>> $expected the corpus's rows
     */
    private function assertBatch(string $label, Batch $batch, array $plan, array $expected, int $seen): void
    {
        $rows = $batch->rows();
        $this->assertGreaterThan(0, $rows, $label);
        $this->assertLessThanOrEqual(\count($expected) - $seen, $rows, "{$label}: more rows than expected");
        $this->assertSame($plan, $batch->columns(), $label);
        foreach ($plan as $column => $declared) {
            $values = $batch->values($column);
            $faults = $batch->faults($column);
            $verdicts = $batch->verdicts($column);
            $this->assertCount($rows, $values, $label);
            $this->assertCount($rows, $verdicts, $label);
            for ($row = 0; $row < $rows; $row++) {
                $at = "{$label}, row " . ($seen + $row) . ", column {$column}";
                $verdict = $batch->get($column, $row);
                $this->assertEquals($verdict, $verdicts[$row], $at);
                if ($verdict instanceof Fault) {
                    $this->assertNull($values[$row], $at);
                    $this->assertEquals($verdict, $faults[$row] ?? null, $at);
                } else {
                    $this->assertArrayNotHasKey($row, $faults, $at);
                    $this->assertEquals($verdict->value, $values[$row], $at);
                }
                $this->assertCell($at, $batch, $declared->door, $column, $row, $expected[$seen + $row][$column]);
            }
        }
    }

    /**
     * Holds one cell of a batch to what the corpus says of it.
     *
     * @param array<string, mixed> $expected the corpus's cell
     */
    private function assertCell(
        string $label,
        Batch $batch,
        Door $door,
        int $column,
        int $row,
        array $expected,
    ): void {
        $verdict = $batch->get($column, $row);
        if ($expected['expect'] !== 'ok') {
            $this->assertInstanceOf(Fault::class, $verdict, $label);
            $this->assertSame(self::REASONS[$expected['expect']], $verdict->reason, $label);
            if (isset($expected['fault'])) {
                $this->assertSame($expected['fault'], [$verdict->offset, $verdict->length], "{$label}: fault span");
                // The cell's own text is still to hand, for the diagnostic a fault deserves.
                $this->assertSame($expected['raw'], $batch->raw($column, $row), "{$label}: raw text");
            } else {
                $this->assertSame([0, 0], [$verdict->offset, $verdict->length], "{$label}: fault span");
            }
            return;
        }

        $this->assertInstanceOf(Success::class, $verdict, $label);
        $value = $verdict->value;
        switch ($door) {
            case Door::Bool:
                $this->assertSame($expected['value'], $value, $label);
                break;
            case Door::I8:
            case Door::I16:
            case Door::I32:
            case Door::I64:
            case Door::U8:
            case Door::U16:
            case Door::U32:
                $this->assertSame($expected['value'], $value, $label);
                break;
            case Door::U64:
                // Beyond PHP_INT_MAX the carrier is the two's-complement bit pattern —
                // compared through the unsigned renderer, the documented consumption path.
                $this->assertIsInt($value, $label);
                $this->assertSame((string) $expected['value'], sprintf('%u', $value), $label);
                break;
            case Door::F32:
                $this->assertSame(unpack('g', pack('g', (float) $expected['value']))[1], $value, $label);
                break;
            case Door::F64:
                $this->assertSame((float) $expected['value'], $value, $label);
                break;
            case Door::Decimal:
                // The triple exactly as the core hands it over, then the canonical rendering.
                $this->assertInstanceOf(Decimal::class, $value, $label);
                $this->assertSame(
                    [(string) $expected['magnitude'], $expected['scale'], $expected['negative']],
                    [$value->magnitude, $value->scale, $value->negative],
                    "{$label}: triple"
                );
                $this->assertSame($expected['value'], (string) $value, "{$label}: canonical text");
                break;
            case Door::Uuid:
                $hex = $expected['value'];
                $this->assertSame(
                    substr($hex, 0, 8) . '-' . substr($hex, 8, 4) . '-' . substr($hex, 12, 4)
                    . '-' . substr($hex, 16, 4) . '-' . substr($hex, 20),
                    $value,
                    $label
                );
                break;
            case Door::Timestamp:
            case Door::Unix:
            case Door::ExcelSerial:
                $this->assertInstant(
                    (new DateTimeImmutable("@{$expected['seconds']}"))
                        ->modify('+' . intdiv($expected['nanos'], 1000) . ' microseconds'),
                    $value,
                    $label
                );
                break;
            case Door::Date:
            case Door::DateOrdered:
                $this->assertInstant(
                    new DateTimeImmutable(
                        sprintf('%04d-%02d-%02d 00:00:00', $expected['year'], $expected['month'], $expected['day']),
                        new DateTimeZone('UTC')
                    ),
                    $value,
                    $label
                );
                break;
            case Door::DateTime:
                $secondOfDay = intdiv($expected['nanos_of_day'], 1_000_000_000);
                $this->assertInstant(
                    new DateTimeImmutable(
                        sprintf(
                            '%04d-%02d-%02d %02d:%02d:%02d.%06d',
                            $expected['year'],
                            $expected['month'],
                            $expected['day'],
                            intdiv($secondOfDay, 3600),
                            intdiv($secondOfDay % 3600, 60),
                            $secondOfDay % 60,
                            intdiv($expected['nanos_of_day'] % 1_000_000_000, 1000)
                        ),
                        new DateTimeZone('UTC')
                    ),
                    $value,
                    $label
                );
                break;
            case Door::Time:
                $this->assertSame($expected['nanos'], $value, $label);
                break;
            case Door::Duration:
                $this->assertEquals(new Duration($expected['seconds'], $expected['nanos']), $value, $label);
                break;
            case Door::Text:
                $this->assertSame($expected['text'], $value, $label);
                // A text cell's raw text is the text.
                $this->assertSame($expected['text'], $batch->raw($column, $row), "{$label}: raw text");
                break;
        }
    }

    /** The same instant to the microsecond, and carried in UTC. */
    private function assertInstant(DateTimeImmutable $expected, mixed $actual, string $label): void
    {
        $this->assertInstanceOf(DateTimeImmutable::class, $actual, $label);
        $this->assertSame($expected->format('Y-m-d\TH:i:s.u'), $actual->format('Y-m-d\TH:i:s.u'), $label);
        $this->assertSame(0, $actual->getOffset(), "{$label}: carried in UTC");
    }

    /** @param array<string, mixed>|null $expected the corpus's failure, if the case has one */
    private function assertFailure(string $label, ?array $expected, ?TabularException $actual): void
    {
        if ($expected === null) {
            $this->assertNull($actual, "{$label}: an unexpected structural failure");
            return;
        }
        $this->assertNotNull($actual, "{$label}: the structural failure was not raised");
        // The corpus names a kind as the core does, in snake case: column_count is ColumnCount.
        $this->assertSame(
            \constant(TabularFailure::class . '::' . str_replace('_', '', ucwords($expected['kind'], '_'))),
            $actual->kind,
            $label
        );
        $this->assertSame(
            [$expected['record'], $expected['line'], $expected['byte']],
            [$actual->record, $actual->recordLine, $actual->byte],
            "{$label}: record, line, byte"
        );
        if (isset($expected['expected'])) {
            $this->assertSame(
                [$expected['expected'], $expected['found']],
                [$actual->expected, $actual->found],
                "{$label}: expected, found"
            );
        }
    }
}
