<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use DateTimeImmutable;
use DateTimeZone;
use HyperCast\CastFailure;
use HyperCast\DateOptions;
use HyperCast\DateOrder;
use HyperCast\Decimal;
use HyperCast\DetailedFault;
use HyperCast\Duration;
use HyperCast\ExcelEpoch;
use HyperCast\Fault;
use HyperCast\MonthNames;
use HyperCast\NumFormat;
use HyperCast\Success;
use HyperCast\UnixPrecision;
use HyperTabular\AmbiguousColumn;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Door;
use HyperTabular\Encoding;
use HyperTabular\Header;
use HyperTabular\LongRows;
use HyperTabular\Origin;
use HyperTabular\Problem;
use HyperTabular\Range;
use HyperTabular\ShortRows;
use HyperTabular\Sheet;
use HyperTabular\SheetOptions;
use HyperTabular\SuggestOptions;
use HyperTabular\Suggestion;
use HyperTabular\SuggestStatus;
use HyperTabular\TabularException;
use HyperTabular\TabularFailure;
use HyperTabular\Tests\Support\Dribble;
use HyperTabular\Workbook;
use HyperTabular\WorkbookFormat;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * Replays the shared conformance corpus (corpus/delimited.json at the repository root) — the
 * same file every binding replays — through this binding: from a string handed over whole, from
 * a stream read through buffers that start at one byte, from a stream that hands back a byte or
 * three a read, and from a file, in batches of one row, of two, and of many. How the input is cut
 * up is the binding's business and must not change the answer. Each of those twice: with the
 * plan handed to the factory, and header first — opened without one, the plan bound once the
 * header has been read. And with notes off, every cell the same less its detail and origin.
 *
 * And corpus/workbook.json, the same way: each package opened from a string, from a stream and
 * from its path, each sheet read by index and (where the name finds it) by name, in batches of
 * one row, of two, and of more than any sheet has — with the plan, and header first. A workbook's
 * buffers are the Rust reader's own, grown by `resize`, so there is no one-element replay of them
 * here: the Rust suite's replay is theirs.
 *
 * Every cell is held to the corpus with its detail and origin, each batch's faults list to the
 * case's `problems`, and a case's `suggest` to what a suggestion says. A key this replay doesn't
 * read fails it: testEveryCorpusKeyIsRead.
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

    private const ENCODINGS = [
        'windows-1252' => Encoding::Windows1252,
        'latin-1' => Encoding::Latin1,
        'utf-16le' => Encoding::Utf16Le,
    ];

    private const DATE_FLAGS = [
        'compact_date' => DateOptions::COMPACT_DATE,
        'space_separator' => DateOptions::SPACE_SEPARATOR,
        'hour_offset' => DateOptions::HOUR_OFFSET,
    ];

    // Every key the replays read, by where it is: what isn't here fails unknownKeys(), so a key
    // added to the corpus can't be passed over. A cell, a problem and a failure compare whole.
    private const CELL_KEYS = [
        'expect', 'value', 'text', 'seconds', 'nanos', 'year', 'month', 'day', 'nanos_of_day',
        'magnitude', 'scale', 'negative', 'fault', 'raw', 'raw_hex', 'detail', 'origin',
    ];
    private const PROBLEM_KEYS = [
        'row', 'line', 'plan_column', 'source_column', 'reason', 'fault', 'detail', 'origin',
        'expected', 'found',
    ];
    private const FAILURE_KEYS = ['kind', 'record', 'line', 'byte', 'expected', 'found'];
    private const PLAN_KEYS = ['door', 'ordinal', 'precision', 'order', 'epoch', 'format', 'tokens', 'options'];
    private const FORMAT_KEYS = ['decimal_sep', 'group_sep', 'flags', 'currency'];
    private const DATE_OPTION_KEYS = ['flags', 'pivot', 'months'];
    private const DELIMITED_KEYS = [
        'name', 'input', 'input_hex', 'dialect', 'plan', 'header', 'rows', 'problems', 'failure', 'suggest',
    ];
    private const DIALECT_KEYS = [
        'separator', 'quoting', 'has_header', 'skip_blank_lines', 'skip', 'header_row', 'short_rows',
        'long_rows', 'tokens', 'encoding',
    ];
    private const SUGGEST_KEYS = ['columns', 'truncated', 'sample_rows', 'format'];
    private const SUGGESTED_KEYS = ['door', 'param', 'status', 'matched', 'sampled'];
    private const WORKBOOK_KEYS = [
        'name', 'file', 'format', 'epoch', 'sheets', 'sheet', 'options', 'plan', 'header', 'numbers',
        'rows', 'problems', 'failure', 'note', 'suggest',
    ];
    private const SHEET_OPTION_KEYS = ['has_header', 'skip_empty_rows', 'range', 'skip', 'header_row', 'tokens'];
    private const SHEET_KEYS = ['name', 'hidden'];

    private const NUMERIC_DOORS = [
        Door::I8, Door::I16, Door::I32, Door::I64, Door::U8, Door::U16, Door::U32, Door::U64,
        Door::F32, Door::F64, Door::Decimal,
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

    /** A case or enum name in the corpus's spelling: `OutOfRange` is `out_of_range`. */
    private static function snake(string $name): string
    {
        return strtolower((string) preg_replace('/(?<!^)[A-Z]/', '_$0', $name));
    }

    /** The case of `$enum` the corpus names in snake case. */
    private static function named(string $enum, string $name): \UnitEnum
    {
        return \constant($enum . '::' . str_replace('_', '', ucwords($name, '_')));
    }

    /**
     * Every key of a case no replay here reads, as a path.
     *
     * @param array<string, mixed> $case
     * @return list<string>
     */
    private static function unknownKeys(array $case, bool $workbook): array
    {
        $name = $case['name'] ?? 'a case with no name';
        $unknown = [];
        $check = static function (mixed $value, string $path, array $known) use (&$unknown): void {
            if (\is_array($value) && !array_is_list($value)) {
                foreach (array_keys($value) as $key) {
                    if (!\in_array($key, $known, true)) {
                        $unknown[] = "{$path}.{$key}";
                    }
                }
            }
        };
        $check($case, $name, $workbook ? self::WORKBOOK_KEYS : self::DELIMITED_KEYS);
        if ($workbook) {
            $check($case['options'] ?? null, "{$name}.options", self::SHEET_OPTION_KEYS);
            foreach ($case['sheets'] ?? [] as $sheet) {
                $check($sheet, "{$name}.sheets[]", self::SHEET_KEYS);
            }
        } else {
            $check($case['dialect'] ?? null, "{$name}.dialect", self::DIALECT_KEYS);
        }
        foreach ($case['plan'] ?? [] as $column) {
            $check($column, "{$name}.plan[]", self::PLAN_KEYS);
            $check($column['format'] ?? null, "{$name}.plan[].format", self::FORMAT_KEYS);
            $check($column['options'] ?? null, "{$name}.plan[].options", self::DATE_OPTION_KEYS);
        }
        foreach ($case['rows'] ?? [] as $row) {
            foreach ($row as $cell) {
                $check($cell, "{$name}.rows[][]", self::CELL_KEYS);
            }
        }
        foreach ($case['problems'] ?? [] as $problem) {
            $check($problem, "{$name}.problems[]", self::PROBLEM_KEYS);
        }
        $check($case['failure'] ?? null, "{$name}.failure", self::FAILURE_KEYS);
        if (isset($case['suggest'])) {
            $check($case['suggest'], "{$name}.suggest", self::SUGGEST_KEYS);
            $check($case['suggest']['format'] ?? null, "{$name}.suggest.format", self::FORMAT_KEYS);
            foreach ($case['suggest']['columns'] as $column) {
                $check($column, "{$name}.suggest.columns[]", self::SUGGESTED_KEYS);
            }
        }
        return $unknown;
    }

    /**
     * Every object in `$value`, the first of each list only, as the keys that reach it.
     *
     * @return list<list<int|string>>
     */
    private static function keyPlaces(mixed $value, array $path = []): array
    {
        if (!\is_array($value) || $value === []) {
            return [];
        }
        if (array_is_list($value)) {
            return self::keyPlaces($value[0], [...$path, 0]);
        }
        $places = [$path];
        foreach ($value as $key => $inner) {
            $places = [...$places, ...self::keyPlaces($inner, [...$path, $key])];
        }
        return $places;
    }

    /**
     * Nothing in the corpus is passed over, and the check notices a key added anywhere.
     *
     * @param list<array<string, mixed>> $cases
     */
    private function assertKeysAreKnown(array $cases, bool $workbook): void
    {
        $unknown = [];
        foreach ($cases as $case) {
            array_push($unknown, ...self::unknownKeys($case, $workbook));
        }
        $this->assertSame([], $unknown);
        foreach ($cases as $case) {
            foreach (self::keyPlaces($case) as $place) {
                $edited = $case;
                $at = &$edited;
                foreach ($place as $step) {
                    $at = &$at[$step];
                }
                $at['junk'] = true;
                unset($at);
                $found = self::unknownKeys($edited, $workbook);
                $this->assertCount(1, $found, json_encode($place));
                $this->assertStringEndsWith('.junk', $found[0]);
            }
        }
    }

    public function testEveryCorpusKeyIsRead(): void
    {
        $this->assertCount(123, self::corpus());
        $this->assertKeysAreKnown(self::corpus(), false);
        $this->assertCount(398, self::corpus('workbook.json'));
        $this->assertKeysAreKnown(self::corpus('workbook.json'), true);
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
    private static function inputOf(array $case): string
    {
        return isset($case['input_hex']) ? hex2bin($case['input_hex']) : $case['input'];
    }

    /** @param array<string, mixed> $entry the case's dialect */
    private static function dialectOf(array $entry): Dialect
    {
        return new Dialect(
            $entry['separator'],
            $entry['quoting'] ?? true,
            $entry['has_header'] ?? true,
            $entry['skip_blank_lines'] ?? true,
            skip: $entry['skip'] ?? 0,
            headerRow: $entry['header_row'] ?? 0,
            shortRows: self::named(ShortRows::class, $entry['short_rows'] ?? 'reject'),
            longRows: self::named(LongRows::class, $entry['long_rows'] ?? 'reject'),
            encoding: isset($entry['encoding']) ? self::ENCODINGS[$entry['encoding']] : Encoding::Utf8,
            tokens: $entry['tokens'] ?? [],
        );
    }

    /**
     * The format an entry names, its flags split as HyperCast's constructor takes them: the
     * lenience flags, and the grouping policy above them. None for an entry with none.
     *
     * @param array<string, mixed> $entry
     */
    private static function formatOf(array $entry): ?NumFormat
    {
        $format = $entry['format'] ?? null;
        if ($format === null) {
            return null;
        }
        return new NumFormat(
            $format['decimal_sep'],
            $format['group_sep'],
            $format['flags'] & 0xFF,
            $format['currency'] ?? '',
            grouping: $format['flags'] & ~0xFF,
        );
    }

    /**
     * A plan entry of the corpus as a Column. An entry with no format builds a column given none,
     * so that the vector says what a column with no format reads.
     *
     * @param array<string, mixed> $entry
     */
    private static function columnOf(array $entry): Column
    {
        $ordinal = $entry['ordinal'];
        $format = self::formatOf($entry);
        $column = match ($entry['door']) {
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
        // Any door has a format, as a suggested plan's do, and only a number's reads it: a factory
        // without one gives the invariant notation, which is all the corpus names for the rest.
        if ($format !== null) {
            if ($format->packed() !== $column->format->packed()) {
                throw new \LogicException("{$entry['door']} with a format a factory can't give it");
            }
        }
        if (isset($entry['tokens'])) {
            $column = $column->withTokens($entry['tokens']);
        }
        if (isset($entry['options'])) {
            $flags = 0;
            foreach ($entry['options']['flags'] as $name) {
                $flags |= self::DATE_FLAGS[$name];
            }
            $months = null;
            if (isset($entry['options']['months'])) {
                $names = [];
                foreach ($entry['options']['months'] as [$month, $name]) {
                    $names[$month][] = $name;
                }
                $months = new MonthNames($names);
            }
            $column = $column->withDateOptions(new DateOptions($flags, $entry['options']['pivot'] ?? 0), $months);
        }
        return $column;
    }

    /** @param list<array<string, mixed>> $problems the case's */
    private static function problemsWithoutNotes(array $problems): array
    {
        $kept = [];
        foreach ($problems as $problem) {
            if (($problem['origin'] ?? null) === 'uncached') {
                continue;
            }
            unset($problem['detail'], $problem['origin']);
            $kept[] = $problem;
        }
        return $kept;
    }

    /** An entry of a batch's faults list in the corpus's shape, its row counted across the read. */
    private function problemJson(Problem $problem, int $base): array
    {
        $entry = ['row' => $base + $problem->row, 'line' => $problem->line];
        if ($problem->planColumn === null) {
            $this->assertNull($problem->sourceColumn);
            $this->assertNull($problem->reason);
            $this->assertSame([0, 0], [$problem->offset, $problem->length]);
            $entry['expected'] = $problem->expected;
            $entry['found'] = $problem->found;
        } else {
            $this->assertNotNull($problem->sourceColumn);
            $this->assertNotNull($problem->reason);
            $this->assertSame([0, 0], [$problem->expected, $problem->found]);
            $entry['plan_column'] = $problem->planColumn;
            $entry['source_column'] = $problem->sourceColumn;
            $entry['reason'] = self::snake($problem->reason->name);
            if ($problem->reason !== CastFailure::Empty) {
                $entry['fault'] = [$problem->offset, $problem->length];
            } else {
                $this->assertSame([0, 0], [$problem->offset, $problem->length]);
            }
        }
        if ($problem->detail !== null) {
            $this->assertSame($problem->detail->value, $problem->detailCode);
            $entry['detail'] = self::snake($problem->detail->name);
        }
        if ($problem->origin !== Origin::Cell) {
            $entry['origin'] = strtolower($problem->origin->name);
        }
        ksort($entry);
        return $entry;
    }

    /** @param list<array<string, mixed>> $problems */
    private static function sorted(array $problems): array
    {
        return array_map(static function (array $problem): array {
            ksort($problem);
            return $problem;
        }, $problems);
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStringHandedOverWhole(array $case): void
    {
        $input = self::inputOf($case);
        foreach (self::BATCH_ROWS as $batchRows) {
            foreach ([false, true] as $headerFirst) {
                $this->replay(
                    "{$case['name']} (string, {$batchRows} rows a batch)",
                    $case,
                    static fn (Dialect $dialect, ?array $plan): DelimitedReader =>
                        DelimitedReader::fromString($input, $dialect, $plan, $batchRows),
                    $headerFirst
                );
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStringWithNotesOff(array $case): void
    {
        $input = self::inputOf($case);
        foreach (self::BATCH_ROWS as $batchRows) {
            $this->replay(
                "{$case['name']} (string, notes off, {$batchRows} rows a batch)",
                $case,
                static fn (Dialect $dialect, ?array $plan): DelimitedReader =>
                    DelimitedReader::fromString($input, $dialect, $plan, $batchRows, notes: false),
                notes: false
            );
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStringDecodedThroughTinyBuffers(array $case): void
    {
        // A buffer matters to a string only under a declared encoding, which decodes through it.
        $input = self::inputOf($case);
        foreach (self::BATCH_ROWS as $batchRows) {
            foreach ([1, 5, 64] as $bufferBytes) {
                foreach ([false, true] as $headerFirst) {
                    $this->replay(
                        "{$case['name']} (string through {$bufferBytes} bytes, {$batchRows} rows a batch)",
                        $case,
                        static fn (Dialect $dialect, ?array $plan): DelimitedReader =>
                            DelimitedReader::fromString($input, $dialect, $plan, $batchRows, $bufferBytes),
                        $headerFirst
                    );
                }
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStreamReadThroughBuffersOfEverySize(array $case): void
    {
        $input = self::inputOf($case);
        foreach (self::BATCH_ROWS as $batchRows) {
            foreach (self::BUFFER_BYTES as $bufferBytes) {
                foreach ([false, true] as $headerFirst) {
                    $stream = fopen('php://memory', 'w+b');
                    fwrite($stream, $input);
                    rewind($stream);
                    try {
                        $this->replay(
                            "{$case['name']} (stream through {$bufferBytes} bytes, {$batchRows} rows a batch)",
                            $case,
                            static fn (Dialect $dialect, ?array $plan): DelimitedReader =>
                                DelimitedReader::fromStream($stream, $dialect, $plan, $batchRows, $bufferBytes),
                            $headerFirst
                        );
                        // The stream is the caller's: the reader left it open.
                        $this->assertIsResource($stream);
                    } finally {
                        fclose($stream);
                    }
                }
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAStreamThatHandsBackAFewBytesARead(array $case): void
    {
        Dribble::register();
        $input = self::inputOf($case);
        foreach (self::BATCH_ROWS as $batchRows) {
            foreach ([2, DelimitedReader::DEFAULT_BUFFER_BYTES] as $bufferBytes) {
                foreach ([false, true] as $headerFirst) {
                    $stream = Dribble::open($input);
                    try {
                        $this->replay(
                            "{$case['name']} (short reads into {$bufferBytes} bytes, {$batchRows} rows a batch)",
                            $case,
                            static fn (Dialect $dialect, ?array $plan): DelimitedReader =>
                                DelimitedReader::fromStream($stream, $dialect, $plan, $batchRows, $bufferBytes),
                            $headerFirst
                        );
                    } finally {
                        fclose($stream);
                    }
                }
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('cases')]
    public function testAFile(array $case): void
    {
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, self::inputOf($case));
        try {
            foreach ([2, 1024] as $batchRows) {
                foreach ([7, DelimitedReader::DEFAULT_BUFFER_BYTES] as $bufferBytes) {
                    foreach ([false, true] as $headerFirst) {
                        $this->replay(
                            "{$case['name']} (file through {$bufferBytes} bytes, {$batchRows} rows a batch)",
                            $case,
                            static fn (Dialect $dialect, ?array $plan): DelimitedReader =>
                                DelimitedReader::open($path, $dialect, $plan, $batchRows, $bufferBytes),
                            $headerFirst
                        );
                    }
                }
            }
        } finally {
            unlink($path);
        }
    }

    /**
     * The reader `$open` builds, held to the case. Header first, it is built without a plan,
     * which is bound once the header has been read and checked.
     *
     * @param array<string, mixed> $case
     * @param callable(Dialect, list<Column>|null): DelimitedReader $open
     */
    private function replay(
        string $label,
        array $case,
        callable $open,
        bool $headerFirst = false,
        bool $notes = true,
    ): void {
        $this->assertSame([], self::unknownKeys($case, false), $label);
        $dialect = self::dialectOf($case['dialect']);
        $plan = array_map(self::columnOf(...), $case['plan']);
        $expected = $case['rows'];

        try {
            if ($headerFirst) {
                $label .= ', header first';
                $reader = $open($dialect, null);
                $this->assertFalse($reader->isBound(), $label);
                $this->assertSame([], $reader->plan(), $label);
                $this->assertHeader($label, $case['header'], $reader->header(), $reader->headerIndex());
                if ($case['header'] !== null && $case['header'] !== []) {
                    $this->assertSame(\count($case['header']), $reader->columnCount(), "{$label}: column count");
                }
                // Reading before a plan is bound is the caller's mistake, and not a lasting one.
                try {
                    $reader->read();
                    $this->fail("{$label}: an unbound reader read");
                } catch (\LogicException $unbound) {
                    $this->assertStringContainsString('bind()', $unbound->getMessage(), $label);
                }
                $reader->bind($plan);
                $this->assertTrue($reader->isBound(), $label);
            } else {
                $reader = $open($dialect, $plan);
            }
        } catch (TabularException $opening) {
            // A failure in the header record itself comes from opening, with no rows.
            $this->assertSame([], $expected, $label);
            $this->assertSame([], $case['problems'], $label);
            $this->assertFailure($label, $case['failure'] ?? null, $opening);
            return;
        }
        $this->assertSame($case['header'], $reader->header(), "{$label}: header");
        $this->assertSame($plan, $reader->plan(), $label);

        $seen = 0;
        $failure = null;
        $problems = [];
        try {
            while (($batch = $reader->read()) !== null) {
                foreach ($batch->faults() as $problem) {
                    $problems[] = $this->problemJson($problem, $seen);
                }
                $this->assertBatch($label, $batch, $plan, $expected, $seen, $notes);
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
        $wanted = $notes ? $case['problems'] : self::problemsWithoutNotes($case['problems']);
        $this->assertSame(self::sorted($wanted), $problems, "{$label}: problems");
        $this->assertFailure($label, $case['failure'] ?? null, $failure);
        $reader->close();
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

    /** @param array<string, mixed> $settings a case's `options` */
    private static function optionsOf(array $settings, int $batchRows, bool $notes = true): SheetOptions
    {
        return new SheetOptions(
            $settings['has_header'],
            $settings['skip_empty_rows'],
            $batchRows,
            skip: $settings['skip'] ?? 0,
            headerRow: $settings['header_row'] ?? 0,
            range: isset($settings['range']) ? Range::parse($settings['range']) : null,
            tokens: $settings['tokens'] ?? [],
            notes: $notes,
        );
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('workbookCases')]
    public function testAWorkbook(array $case): void
    {
        $this->assertSame([], self::unknownKeys($case, true), $case['name']);
        $path = self::directory() . '/' . $case['file'];
        $openings = [
            'string' => static fn (): Workbook => Workbook::fromString(file_get_contents($path)),
            'stream' => function () use ($path): Workbook {
                $stream = fopen($path, 'rb');
                try {
                    return Workbook::fromStream($stream);
                } finally {
                    // The stream is the caller's: the workbook read it to its end and left it open.
                    $this->assertIsResource($stream);
                    $this->assertTrue(feof($stream));
                    fclose($stream);
                }
            },
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
            $this->assertSame(match ($case['format']) {
                'xlsx' => WorkbookFormat::Xlsx,
                'numbers' => WorkbookFormat::Numbers,
                default => WorkbookFormat::Ods,
            }, $book->format());
            $this->assertSame(ExcelEpoch::from($case['epoch']), $book->dateSystem());
            $this->assertSame(
                array_map(static fn (array $sheet): array => [$sheet['name'], $sheet['hidden']], $case['sheets']),
                array_map(static fn ($sheet): array => [$sheet->name, $sheet->hidden], $book->sheets())
            );
            $byName = array_search($name, array_map(static fn ($sheet): string => $sheet->name, $book->sheets()), true)
                === $index;
            foreach (self::BATCH_ROWS as $batchRows) {
                $options = self::optionsOf($case['options'], $batchRows);
                foreach ($byName ? [$index, $name] : [$index] as $which) {
                    foreach ([false, true] as $headerFirst) {
                        $this->replaySheet(
                            "{$case['name']}: {$source}, {$batchRows} rows a batch, sheet " . var_export($which, true),
                            $case,
                            $plan,
                            $headerFirst
                                ? static fn (): Sheet => $book->sheet($which, $options)
                                : static fn (): Sheet => $book->sheet($which, $options, $plan),
                            $headerFirst
                        );
                    }
                }
            }
        }
    }

    /** @param array<string, mixed> $case */
    #[DataProvider('workbookCases')]
    public function testAWorkbookWithNotesOff(array $case): void
    {
        if (!isset($case['sheet'])) {
            $this->addToAssertionCount(1);
            return;
        }
        $plan = array_map(self::columnOf(...), $case['plan']);
        $book = Workbook::open(self::directory() . '/' . $case['file']);
        foreach (self::BATCH_ROWS as $batchRows) {
            $options = self::optionsOf($case['options'], $batchRows, false);
            $this->replaySheet(
                "{$case['name']}: notes off, {$batchRows} rows a batch",
                $case,
                $plan,
                static fn (): Sheet => $book->sheet($case['sheet'], $options, $plan),
                false,
                false
            );
        }
    }

    /**
     * A batch's faults, asked after every later read, are those it had when it was read: a
     * sheet's source columns and row numbers and a formula's missing cache among them.
     *
     * @param array<string, mixed> $case
     */
    #[DataProvider('workbookCases')]
    public function testASheetsBatchHeldPastTheNextReadStillAnswersForItself(array $case): void
    {
        if (!isset($case['sheet'])) {
            $this->addToAssertionCount(1);
            return;
        }
        $plan = array_map(self::columnOf(...), $case['plan']);
        $book = Workbook::open(self::directory() . '/' . $case['file']);
        foreach ([1, 3] as $batchRows) {
            $batches = function () use ($book, $case, $plan, $batchRows): array {
                $sheet = $book->sheet($case['sheet'], self::optionsOf($case['options'], $batchRows), $plan);
                $read = [];
                try {
                    while (($batch = $sheet->read()) !== null) {
                        $read[] = $batch;
                    }
                } catch (TabularException) {
                    // What came before the failure is what this holds.
                }
                return $read;
            };
            $atOnce = [];
            $sheet = $book->sheet($case['sheet'], self::optionsOf($case['options'], $batchRows), $plan);
            try {
                while (($batch = $sheet->read()) !== null) {
                    $atOnce[] = $batch->faults();
                }
            } catch (TabularException) {
                // As above.
            }
            $held = array_map(static fn (Batch $batch): array => $batch->faults(), $batches());
            $this->assertEquals($atOnce, $held, $case['name']);
            $this->assertCount(\count($case['problems']), array_merge([], ...$held), $case['name']);
        }
    }

    /**
     * The sheet `$open` opens, held to the case. Header first, it is opened without a plan,
     * which is bound once the header has been read and checked.
     *
     * @param array<string, mixed> $case
     * @param list<Column> $plan the case's plan
     * @param callable(): Sheet $open
     */
    private function replaySheet(
        string $label,
        array $case,
        array $plan,
        callable $open,
        bool $headerFirst,
        bool $notes = true,
    ): void {
        $sheet = $open();
        if ($headerFirst) {
            $label .= ', header first';
            $this->assertFalse($sheet->isBound(), $label);
            $this->assertSame([], $sheet->plan(), $label);
            $this->assertHeader($label, $case['header'], $sheet->header(), $sheet->headerIndex());
            try {
                $sheet->read();
                $this->fail("{$label}: an unbound sheet read");
            } catch (\LogicException $unbound) {
                $this->assertStringContainsString('bind()', $unbound->getMessage(), $label);
            }
            $sheet->bind($plan);
            $this->assertTrue($sheet->isBound(), $label);
            try {
                $sheet->bind($plan);
                $this->fail("{$label}: bound twice");
            } catch (\LogicException $twice) {
                $this->assertStringContainsString('bound once', $twice->getMessage(), $label);
            }
        }
        $batchRows = $sheet->options()->batchRows;
        $this->assertSame($case['header'], $sheet->header(), "{$label}: header");
        $this->assertSame($plan, $sheet->plan(), $label);
        $seen = 0;
        $failure = null;
        $problems = [];
        try {
            while (($batch = $sheet->read()) !== null) {
                $this->assertLessThanOrEqual($batchRows, $batch->rows(), $label);
                for ($row = 0; $row < $batch->rows(); $row++) {
                    $this->assertSame($case['numbers'][$seen + $row], $batch->line($row), "{$label}, row");
                }
                foreach ($batch->faults() as $problem) {
                    $problems[] = $this->problemJson($problem, $seen);
                }
                $this->assertBatch($label, $batch, $plan, $case['rows'], $seen, $notes);
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
        $this->assertCount($seen, $case['numbers'], $label);
        $wanted = $notes ? $case['problems'] : self::problemsWithoutNotes($case['problems']);
        $this->assertSame(self::sorted($wanted), $problems, "{$label}: problems");
        $this->assertFailure($label, $case['failure'] ?? null, $failure);
    }

    /**
     * Holds one batch to the corpus's rows from `$seen` on, column by column: the column as a
     * whole, then each cell of it, the same answer both ways, and each of its rows as a `Row`.
     *
     * @param list<Column> $plan the plan the batch was read through
     * @param list<list<array<string, mixed>>> $expected the corpus's rows
     */
    private function assertBatch(
        string $label,
        Batch $batch,
        array $plan,
        array $expected,
        int $seen,
        bool $notes,
    ): void {
        $rows = $batch->rows();
        $this->assertGreaterThan(0, $rows, $label);
        $this->assertLessThanOrEqual(\count($expected) - $seen, $rows, "{$label}: more rows than expected");
        $this->assertSame($plan, $batch->columns(), $label);
        for ($row = 0; $row < $rows; $row++) {
            $this->assertCount(\count($plan), $expected[$seen + $row], "{$label}: a corpus row of another width");
        }
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
                $this->assertCell(
                    $at,
                    $batch,
                    $declared->door,
                    $column,
                    $row,
                    $expected[$seen + $row][$column],
                    $notes
                );
            }
        }
        // The row view: every row, in order, each answering as the batch does for it.
        $index = 0;
        foreach ($batch as $key => $row) {
            $this->assertSame($index, $key, $label);
            $this->assertSame($index, $row->index(), $label);
            $this->assertSame($batch->line($index), $row->line(), $label);
            foreach (array_keys($plan) as $column) {
                $this->assertEquals($batch->get($column, $index), $row->get($column), $label);
                $this->assertSame($batch->raw($column, $index), $row->raw($column), $label);
                $this->assertSame($batch->origin($column, $index), $row->origin($column), $label);
                $this->assertEquals($batch->detail($column, $index), $row->detail($column), $label);
            }
            $index++;
        }
        $this->assertSame($rows, $index, $label);
    }

    /**
     * Holds one cell of a batch to what the corpus says of it: its verdict and value, its detail
     * and its origin.
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
        bool $notes,
    ): void {
        $verdict = $batch->get($column, $row);
        $origin = $notes ? ($expected['origin'] ?? 'cell') : 'cell';
        $this->assertSame(self::named(Origin::class, $origin), $batch->origin($column, $row), "{$label}: origin");
        $detail = $batch->detail($column, $row);
        if ($expected['expect'] !== 'ok') {
            $this->assertInstanceOf(Fault::class, $verdict, $label);
            $this->assertSame(self::REASONS[$expected['expect']], $verdict->reason, $label);
            if (isset($expected['fault'])) {
                $this->assertSame($expected['fault'], [$verdict->offset, $verdict->length], "{$label}: fault span");
                // The cell's own text is still to hand, for the diagnostic a fault deserves.
                $raw = isset($expected['raw_hex']) ? hex2bin($expected['raw_hex']) : $expected['raw'];
                $this->assertSame($raw, $batch->raw($column, $row), "{$label}: raw text");
            } else {
                $this->assertSame([0, 0], [$verdict->offset, $verdict->length], "{$label}: fault span");
            }
            if ($expected['expect'] === 'empty' && !isset($expected['origin'])) {
                $this->assertSame('', $batch->raw($column, $row), "{$label}: raw text");
            }
            // What failed in the cell, as HyperCast's DetailedFault of the same reason and span.
            $said = $notes ? ($expected['detail'] ?? null) : null;
            if ($said === null) {
                $this->assertNull($detail, "{$label}: detail");
            } else {
                $this->assertInstanceOf(DetailedFault::class, $detail, "{$label}: detail");
                $this->assertSame($said, self::snake($detail->detail->name), "{$label}: detail");
                $this->assertSame($detail->detail->value, $detail->code, "{$label}: detail");
                $this->assertEquals($verdict, $detail->toFault(), "{$label}: detail");
            }
            return;
        }
        $this->assertNull($detail, "{$label}: detail");

        $this->assertInstanceOf(Success::class, $verdict, $label);
        $value = $verdict->value;
        switch ($door) {
            case Door::Bool:
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

    /**
     * A header as the case states it, and its {@see Header} the same names: a name one column
     * has found strictly, and a name two have refused by the strict lookup and found first by
     * the optional one.
     *
     * @param list<string>|null $expected the case's header
     * @param list<string>|null $names the reader's
     */
    private function assertHeader(string $label, ?array $expected, ?array $names, ?Header $header): void
    {
        $this->assertSame($expected, $names, "{$label}: header");
        if ($expected === null) {
            $this->assertNull($header, "{$label}: header index");
            return;
        }
        $this->assertNotNull($header, "{$label}: header index");
        $this->assertSame($expected, $header->names(), "{$label}: header index");
        $this->assertCount(\count($expected), $header, "{$label}: header index");
        $counts = array_count_values($expected);
        foreach ($expected as $ordinal => $name) {
            $first = array_search($name, $expected, true);
            $this->assertSame($first, $header->find($name), "{$label}: {$name}");
            if ($counts[$name] === 1) {
                $this->assertSame($first, $header->ordinal($name), "{$label}: {$name}");
            } else {
                try {
                    $header->ordinal($name);
                    $this->fail("{$label}: {$name}, which two columns have, was resolved");
                } catch (AmbiguousColumn $twice) {
                    $this->assertSame($first, $twice->first, "{$label}: {$name}");
                }
            }
            $this->assertSame($name, $header[$ordinal], "{$label}: {$name}");
        }
    }

    /** The same instant to the microsecond, and carried in UTC. */
    private function assertInstant(DateTimeImmutable $expected, mixed $actual, string $label): void
    {
        $this->assertInstanceOf(DateTimeImmutable::class, $actual, $label);
        $this->assertSame($expected->format('Y-m-d\TH:i:s.u'), $actual->format('Y-m-d\TH:i:s.u'), $label);
        $this->assertSame(0, $actual->getOffset(), "{$label}: carried in UTC");
    }

    /**
     * The whole failure, an absent `expected` and `found` being 0.
     *
     * @param array<string, mixed>|null $expected the corpus's failure, if the case has one
     */
    private function assertFailure(string $label, ?array $expected, ?TabularException $actual): void
    {
        if ($expected === null) {
            $this->assertNull($actual, "{$label}: an unexpected structural failure");
            return;
        }
        $this->assertNotNull($actual, "{$label}: the structural failure was not raised");
        // The corpus names a kind as the core does, in snake case: column_count is ColumnCount.
        $this->assertSame(self::named(TabularFailure::class, $expected['kind']), $actual->kind, $label);
        $this->assertSame(
            [
                $expected['record'],
                $expected['line'],
                $expected['byte'],
                $expected['expected'] ?? 0,
                $expected['found'] ?? 0
            ],
            [$actual->record, $actual->recordLine, $actual->byte, $actual->expected, $actual->found],
            "{$label}: record, line, byte, expected, found"
        );
    }

    /**
     * A report in the corpus's shape. An ambiguous column's `param` is the set of its candidates,
     * and a resolved number's is left at 0, since a report names the format the plan uses and
     * can't say what `param` was.
     *
     * @return list<array<string, mixed>>
     */
    private static function suggestionJson(Suggestion $found): array
    {
        $columns = [];
        foreach ($found->report as $column) {
            $param = 0;
            if ($column->candidates !== []) {
                foreach ($column->candidates as $candidate) {
                    $param |= $candidate instanceof DateOrder
                        ? 1 << $candidate->value
                        : ($candidate->decimalSep === '.' ? 2 : 4);
                }
            } elseif ($column->door === Door::DateOrdered || $column->door === Door::DateTime) {
                $param = $column->declared->value;
            }
            $columns[] = [
                'door' => self::snake($column->door->name),
                'matched' => $column->matched,
                'param' => $param,
                'sampled' => $column->sampled,
                'status' => strtolower($column->status->name),
            ];
        }
        return $columns;
    }

    /**
     * What a report should say of the corpus's `columns`: see suggestionJson().
     *
     * @param list<array<string, mixed>> $columns
     */
    private static function reported(array $columns): array
    {
        return array_map(static function (array $column): array {
            $dated = \in_array($column['door'], ['date_ordered', 'datetime'], true);
            if ($column['status'] === 'resolved' && !$dated) {
                $column['param'] = 0;
            }
            ksort($column);
            return $column;
        }, $columns);
    }

    /**
     * Each resolved number is planned and reported in the format the corpus's `param` says, or the
     * one declared, which is the caller's own object; anything else reports none.
     *
     * @param list<array<string, mixed>> $columns
     */
    private function assertNotation(string $label, Suggestion $found, array $columns, ?NumFormat $declared): void
    {
        foreach ($found->report as $at => $reported) {
            $planned = $found->plan[$at];
            if (
                $reported->status === SuggestStatus::Resolved && \in_array($reported->door, self::NUMERIC_DOORS, true)
            ) {
                $notation = $columns[$at]['param'] === 4 ? new NumFormat(',', '.', NumFormat::ALL) : null;
                $wanted = $declared ?? $notation ?? NumFormat::invariant();
                $this->assertNotNull($reported->format, $label);
                $this->assertSame($wanted->packed(), $reported->format->packed(), $label);
                $this->assertSame($wanted->packed(), $planned->format->packed(), $label);
                if ($declared !== null) {
                    $this->assertSame($declared, $reported->format, "{$label}: the caller's own object");
                }
            } else {
                $this->assertNull($reported->format, $label);
            }
        }
    }

    /** @return iterable<string, array{array<string, mixed>}> */
    public static function suggestingCases(): iterable
    {
        foreach (self::corpus() as $case) {
            if (isset($case['suggest'])) {
                yield $case['name'] => [$case];
            }
        }
    }

    public function testTheCorpusSaysWhatToSuggest(): void
    {
        $this->assertCount(19, iterator_to_array(self::suggestingCases()));
        $this->assertCount(15, iterator_to_array(self::suggestingWorkbookCases()));
    }

    /**
     * A suggestion from a string and from a file says what the corpus does, and its plan reads
     * back refusing nothing it resolved among the rows it sampled.
     *
     * @param array<string, mixed> $case
     */
    #[DataProvider('suggestingCases')]
    public function testEverySuggestionIsWhatTheCorpusSays(array $case): void
    {
        $label = $case['name'];
        $expected = $case['suggest'];
        $data = self::inputOf($case);
        $dialect = self::dialectOf($case['dialect']);
        $declared = self::formatOf($expected);
        $asked = new SuggestOptions($expected['sample_rows'] ?? 1000, $declared);
        $found = DelimitedReader::suggestString($data, $dialect, $asked);
        $this->assertSame(self::reported($expected['columns']), self::suggestionJson($found), $label);
        $this->assertSame($expected['truncated'], $found->truncated, $label);
        $this->assertNotation($label, $found, $expected['columns'], $declared);
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, $data);
        try {
            $this->assertEquals($found, DelimitedReader::suggest($path, $dialect, $asked), $label);
        } finally {
            unlink($path);
        }

        $sample = $expected['sample_rows'] ?? \count($case['rows']);
        $read = 0;
        $reader = DelimitedReader::fromString($data, $dialect, $found->plan);
        try {
            while (($batch = $reader->read()) !== null) {
                foreach ($batch->faults() as $problem) {
                    $column = $problem->planColumn;
                    $refused = $column !== null
                        && $found->report[$column]->status === SuggestStatus::Resolved
                        && $found->report[$column]->door !== Door::Text
                        && $problem->row + $read < $sample;
                    $this->assertFalse($refused, $label);
                }
                $read += $batch->rows();
            }
        } catch (TabularException) {
            $this->assertGreaterThanOrEqual($sample, $read, $label);
        }
    }

    public function testSuggestFailsOnlyWhereAReadOfTheCaseFails(): void
    {
        $planned = $failed = 0;
        foreach (self::corpus() as $case) {
            try {
                DelimitedReader::suggestString(self::inputOf($case), self::dialectOf($case['dialect']));
                $planned++;
            } catch (TabularException) {
                $failed++;
                $this->assertArrayHasKey('failure', $case, $case['name']);
            }
        }
        $this->assertGreaterThan(80, $planned);
        $this->assertGreaterThan(0, $failed);
    }

    /** @return iterable<string, array{array<string, mixed>}> */
    public static function suggestingWorkbookCases(): iterable
    {
        foreach (self::corpus('workbook.json') as $case) {
            if (isset($case['suggest'])) {
                yield $case['name'] => [$case];
            }
        }
    }

    /**
     * Through the sheet, and a plan that then reads back refusing nothing.
     *
     * @param array<string, mixed> $case
     */
    #[DataProvider('suggestingWorkbookCases')]
    public function testEverySheetSuggestionIsWhatTheCorpusSays(array $case): void
    {
        $label = $case['name'];
        $expected = $case['suggest'];
        $declared = self::formatOf($expected);
        $options = self::optionsOf($case['options'], DelimitedReader::DEFAULT_BATCH_ROWS);
        $book = Workbook::open(self::directory() . '/' . $case['file']);
        $found = $book->sheet($case['sheet'], $options)->suggest(new SuggestOptions(format: $declared));
        $this->assertSame(self::reported($expected['columns']), self::suggestionJson($found), $label);
        $this->assertSame($expected['truncated'], $found->truncated, $label);
        $this->assertNotation($label, $found, $expected['columns'], $declared);
        $sheet = $book->sheet($case['sheet'], $options, $found->plan);
        while (($batch = $sheet->read()) !== null) {
            foreach ($batch->faults() as $problem) {
                $this->assertNotContains(
                    $problem->reason,
                    [CastFailure::Malformed, CastFailure::OutOfRange],
                    $label
                );
            }
        }
    }
}
