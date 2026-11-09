<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperCast\CastFailure;
use HyperCast\FaultDetail;
use HyperCast\NumFormat;
use HyperCast\Success;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Door;
use HyperTabular\SheetOptions;
use HyperTabular\SuggestOptions;
use HyperTabular\SuggestStatus;
use HyperTabular\TabularException;
use HyperTabular\Workbook;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\Attributes\RequiresPhp;
use PHPUnit\Framework\TestCase;

/**
 * A column's grouping: Western unless the caller declares Indian on that column, in the same read
 * as a column that doesn't, and a stale policy word refused where it's handed over, before any
 * cell. What the shapes read is HyperCast's and the corpus's. No test here names a locale: the
 * Indian shape is declared, never looked up.
 *
 * HyperCast's constructor and its `unserialize` refuse a stale word, so a format holding one is
 * forged as HyperCast's own GroupingTest forges it: an instance made without its constructor, its
 * readonly slots written by reflection. HyperCast's `packed()` refuses such a format, and that is
 * the door every bind, open and suggest goes through here: the refusal is this binding's argument
 * error, HyperCast's exception kept as its previous one. NumFormat is final, so nothing that packs
 * a stale word gets past `packed()` to the core's own judge; the Rust suite proves that one.
 */
#[RequiresPhp('>= 8.4.0')]
final class GroupingTest extends TestCase
{
    private const BOTH = "indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n";

    private static function indian(): NumFormat
    {
        return NumFormat::invariant()->withGrouping(NumFormat::GROUPING_INDIAN);
    }

    /** The packages that hold 1,23,456 and 1,234,567 as text, and 1234 typed, in column C. */
    private static function packages(): array
    {
        return array_map(
            static fn (string $kind): string => \dirname(__DIR__, 2) . "/corpus/workbook/grouping.{$kind}",
            ['xlsx', 'ods', 'numbers']
        );
    }

    private static function broken(): string
    {
        return \dirname(__DIR__, 2) . '/corpus/workbook/broken.xlsx';
    }

    /** The word a format packs: its lenience flags and grouping policy, as the core reads them. */
    private static function word(NumFormat $format): int
    {
        return unpack('V', $format->packed(), 4)[1];
    }

    /**
     * A NumFormat whose flags word is `$word`: HyperCast's own where it makes one, forged where it
     * refuses to.
     */
    private static function format(int $word): NumFormat
    {
        $made = match ($word) {
            95 => NumFormat::invariant(),
            607 => self::indian(),
            default => null,
        };
        if ($made !== null) {
            return $made;
        }
        if (!self::forgeable()) {
            self::markTestSkipped(self::UNFORGEABLE);
        }
        $sound = NumFormat::invariant();
        $forged = (new \ReflectionClass(NumFormat::class))->newInstanceWithoutConstructor();
        foreach ((new \ReflectionClass(NumFormat::class))->getProperties() as $slot) {
            $value = match ($slot->getName()) {
                'flags' => $word & 0xFF,
                'grouping' => $word & ~0xFF,
                default => $sound->{$slot->getName()},
            };
            $slot->setRawValueWithoutLazyInitialization($forged, $value);
        }
        return $forged;
    }

    /**
     * PHP 8.3 has no way to write an internal class's readonly slot from outside
     * (`setRawValueWithoutLazyInitialization` is PHP 8.4's), so there no stale word can be made, which
     * is the protection itself: HyperCast refuses one at construction and at `unserialize`.
     */
    private const UNFORGEABLE = 'PHP 8.3 cannot forge a format: no stale word can reach a plan there';

    private static function forgeable(): bool
    {
        return method_exists(\ReflectionProperty::class, 'setRawValueWithoutLazyInitialization');
    }

    /** @return iterable<string, array{int, bool}> */
    public static function words(): iterable
    {
        yield '351' => [351, true];
        yield '863' => [863, true];
        yield '95' => [95, false];
        yield '607' => [607, false];
    }

    /** @return list<Column> */
    private static function planned(NumFormat $format): array
    {
        return [Column::i32(0), Column::i32(1, $format)];
    }

    /** `$call` throws this binding's refusal naming `$what` when `$refused`, and nothing otherwise. */
    private function rejects(bool $refused, callable $call, string $what = "Plan column 1's format"): void
    {
        if (!$refused) {
            $call();
            $this->addToAssertionCount(1);
            return;
        }
        try {
            $call();
            $this->fail("{$what}: a stale word was taken");
        } catch (\InvalidArgumentException $refusal) {
            $this->assertSame("{$what} cannot be honoured", $refusal->getMessage());
            // What HyperCast said of it, kept.
            $this->assertInstanceOf(\RuntimeException::class, $refusal->getPrevious());
        }
    }

    /** Indian in one column, Western in the other. */
    private function holdsBoth(Batch $batch, int $indian, int $western): void
    {
        $this->assertEquals(new Success(123_456), $batch->get($indian, 0));
        $this->assertSame(CastFailure::Malformed, $batch->get($western, 0)->reason);
        $this->assertSame(FaultDetail::GroupSize, $batch->detail($western, 0)?->detail);
        $this->assertSame(CastFailure::Malformed, $batch->get($indian, 1)->reason);
        $this->assertEquals(new Success(1_234_567), $batch->get($western, 1));
    }

    public function testTheFormsStandInForExactlyWhatTheRealFormatsPack(): void
    {
        $this->assertSame(95, self::word(NumFormat::invariant()));
        $this->assertSame(607, self::word(self::indian()));
        $this->assertSame(95, self::word(Column::i32(1)->format));
        if (!self::forgeable()) {
            return;
        }
        // The forging is sound where its word is: a forged 607 packs as HyperCast's own and reads.
        $sound = (new \ReflectionClass(NumFormat::class))->newInstanceWithoutConstructor();
        foreach ((new \ReflectionClass(NumFormat::class))->getProperties() as $slot) {
            $slot->setRawValueWithoutLazyInitialization($sound, self::indian()->{$slot->getName()});
        }
        $this->assertSame(self::indian()->packed(), $sound->packed());
        $batch = DelimitedReader::fromString(self::BOTH, Dialect::csv(), [Column::i32(0, $sound)])->read();
        $this->assertEquals(new Success(123_456), $batch->get(0, 0));
        // And a forged stale word is what HyperCast's packed() refuses.
        foreach ([351, 863] as $word) {
            try {
                self::format($word)->packed();
                $this->fail("{$word} packed");
            } catch (\RuntimeException) {
                $this->addToAssertionCount(1);
            }
        }
    }

    public function testHyperCastItselfWillNotMakeAStaleWord(): void
    {
        foreach ([351, 863] as $word) {
            try {
                new NumFormat('.', ',', $word & 0xFF, grouping: $word & ~0xFF);
                $this->fail("{$word} constructed");
            } catch (\InvalidArgumentException) {
                $this->addToAssertionCount(1);
            }
        }
    }

    public function testAnIndianColumnReadsBesideAWesternOne(): void
    {
        $plan = [Column::i32(0, self::indian()), Column::i32(1)];
        $this->holdsBoth(DelimitedReader::fromString(self::BOTH, Dialect::csv(), $plan)->read(), 0, 1);
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, self::BOTH);
        rewind($stream);
        $this->holdsBoth(DelimitedReader::fromStream($stream, Dialect::csv(), $plan)->read(), 0, 1);
        fclose($stream);
    }

    public function testAnIndianSheetColumnReadsBesideAWesternOne(): void
    {
        foreach (self::packages() as $path) {
            $batch = Workbook::open($path)
                ->sheet(0, new SheetOptions(), [Column::i32(2, self::indian()), Column::i32(2)])
                ->read();
            $this->holdsBoth($batch, 0, 1);
            // A typed number has no notation: it's the same under either.
            $this->assertEquals(new Success(1234), $batch->get(0, 2), $path);
            $this->assertEquals(new Success(1234), $batch->get(1, 2), $path);
        }
    }

    #[DataProvider('words')]
    public function testAStalePolicyIsRefusedAtEveryDelimitedEntryPoint(int $word, bool $refused): void
    {
        $plan = self::planned(self::format($word));
        $path = tempnam(sys_get_temp_dir(), 'hypertabular');
        file_put_contents($path, self::BOTH);
        $stream = fopen('php://memory', 'w+b');
        fwrite($stream, self::BOTH);
        rewind($stream);
        try {
            $this->rejects($refused, static fn () => DelimitedReader::fromString(self::BOTH, Dialect::csv(), $plan));
            $this->rejects($refused, static fn () => DelimitedReader::fromStream($stream, Dialect::csv(), $plan));
            $this->rejects($refused, static fn () => DelimitedReader::open($path, Dialect::csv(), $plan));
        } finally {
            fclose($stream);
            unlink($path);
        }
        $unbound = DelimitedReader::fromString(self::BOTH, Dialect::csv());
        $records = $unbound->records();
        $this->rejects($refused, static fn () => $unbound->bind($plan));
        $this->assertSame(!$refused, $unbound->isBound());
        if ($refused) {
            // Nothing bound, and no cell read.
            $this->assertSame($records, $unbound->records());
            $this->expectException(\LogicException::class);
            $unbound->read();
        }
    }

    #[DataProvider('words')]
    public function testAStalePolicyIsRefusedAtEverySheetEntryPoint(int $word, bool $refused): void
    {
        $book = Workbook::open(self::packages()[0]);
        $plan = self::planned(self::format($word));
        $this->rejects($refused, static fn () => $book->sheet(0, new SheetOptions(), $plan));
        $unbound = $book->sheet(0, new SheetOptions());
        $this->rejects($refused, static fn () => $unbound->bind($plan));
        $this->assertSame(!$refused, $unbound->isBound());
        if ($refused) {
            $this->expectException(\LogicException::class);
            $unbound->read();
        }
    }

    /**
     * A suggest judges every format it's given before it opens anything: one never met, one
     * replaced, one past the sample, an empty source and a broken one. The sound words are the
     * control, each getting as far as the source does.
     */
    #[DataProvider('words')]
    public function testAStaleFormatIsRefusedBeforeASuggestOpensAnything(int $word, bool $refused): void
    {
        $format = self::format($word);
        $headerless = new Dialect(',', hasHeader: false);
        $every = new SuggestOptions(format: $format);
        $past = new SuggestOptions(columnFormats: [99 => $format]);
        $replaced = new SuggestOptions(format: $format, columnFormats: [0 => self::indian()]);
        $whole = "SuggestOptions' format";
        foreach (
            [
                ["a\n1\n", Dialect::csv(), $every, $whole],
                ['', Dialect::csv(), $every, $whole],
                ['', $headerless, $every, $whole],
                ["a\n1\n", Dialect::csv(), $past, "SuggestOptions' columnFormats[99]"],
                ["a\n1\n", Dialect::csv(), $replaced, $whole],
            ] as [$data, $dialect, $options, $what]
        ) {
            $this->rejects($refused, static fn () => DelimitedReader::suggestString($data, $dialect, $options), $what);
        }

        // A broken source says so only of a format that's sound.
        $failsAs = function (string $error, callable $call) use ($refused, $whole): void {
            try {
                $call();
                $this->fail('a broken source suggested');
            } catch (\Throwable $thrown) {
                if ($refused) {
                    $this->assertInstanceOf(\InvalidArgumentException::class, $thrown);
                    $this->assertSame("{$whole} cannot be honoured", $thrown->getMessage());
                } else {
                    $this->assertInstanceOf($error, $thrown);
                }
            }
        };
        $failsAs(
            TabularException::class,
            static fn () => DelimitedReader::suggestString("a\n\"1\n", Dialect::csv(), $every)
        );
        $failsAs(TabularException::class, static fn () => DelimitedReader::suggestString("\"1\n", $headerless, $every));
        $failsAs(
            \RuntimeException::class,
            static fn () => @DelimitedReader::suggest('/no/such/file.csv', Dialect::csv(), $every)
        );

        // A sheet: the same, and one whose sample would meet its broken part.
        $sheet = Workbook::open(self::packages()[0])->sheet(0, new SheetOptions());
        $asked = [[$every, $whole], [$past, "SuggestOptions' columnFormats[99]"], [$replaced, $whole]];
        foreach ($asked as [$options, $what]) {
            $this->rejects($refused, static fn () => $sheet->suggest($options), $what);
        }
        $broken = Workbook::open(self::broken())->sheet(0, new SheetOptions());
        $failsAs(TabularException::class, static fn () => $broken->suggest($every));
    }

    public function testAFormatIsTakenAheadOfIndianOnTheSameColumn(): void
    {
        $indian = self::indian();
        $twice = new SuggestOptions(format: NumFormat::invariant(), columnFormats: [0 => $indian]);
        $this->assertSame($indian, DelimitedReader::suggestString("a\n7\n", Dialect::csv(), $twice)->report[0]->format);
    }

    /** What the delimiter rule says stays as it was: nothing says `1,23,456` is Indian. */
    public function testAnIndianShapeHasNoCultureUntilOneIsDeclared(): void
    {
        $ask = static fn (string $data, ?SuggestOptions $options = null) =>
            DelimitedReader::suggestString($data, Dialect::csv(), $options);

        $found = $ask("a\n\"1,23,456\"\n");
        $this->assertSame([Door::Text, SuggestStatus::Resolved], [$found->report[0]->door, $found->report[0]->status]);
        $found = $ask("a\n\"1,23,456\"\n5\n");
        $this->assertSame(
            [Door::Text, SuggestStatus::Unresolved],
            [$found->report[0]->door, $found->report[0]->status]
        );
        // A suggested plan is Western: flags 95, and a later Indian shape is refused.
        $found = $ask("a\n\"1,234,567\"\n");
        $this->assertSame(NumFormat::invariant()->packed(), $found->report[0]->format->packed());
        $this->assertSame(95, self::word($found->plan[0]->format));
        $batch = DelimitedReader::fromString("a\n\"1,23,456\"\n", Dialect::csv(), $found->plan)->read();
        $this->assertSame(CastFailure::Malformed, $batch->get(0, 0)->reason);

        // Declared, the column is a number, its plan reads it, and a Western shape is what's refused.
        $indian = self::indian();
        $declared = new SuggestOptions(columnFormats: [0 => $indian]);
        $both = "a,b\n\"1,23,456\",\"1,23,456\"\n";
        $found = $ask($both, $declared);
        $this->assertSame([Door::I32, SuggestStatus::Resolved], [$found->report[0]->door, $found->report[0]->status]);
        $this->assertSame($indian, $found->report[0]->format);
        $this->assertSame(607, self::word($found->plan[0]->format));
        $batch = DelimitedReader::fromString($both, Dialect::csv(), $found->plan)->read();
        $this->assertEquals(new Success(123_456), $batch->get(0, 0));
        // The other column asked nothing declared, and stays text.
        $this->assertSame(Door::Text, $found->report[1]->door);
        $found = $ask("a\n\"1,234,567\"\n", $declared);
        $this->assertSame(
            [Door::Text, SuggestStatus::Resolved, null],
            [$found->report[0]->door, $found->report[0]->status, $found->report[0]->format]
        );
    }

    /** No format, the invariant one and a format of the invariant's fields all read the same. */
    public function testADefaultIsWesternHoweverItIsReached(): void
    {
        $data = "a\n\"1,234,567\"\n\"1,23,456\"\n";
        $fields = new NumFormat('.', ',', NumFormat::ALL);
        foreach ([Column::i32(0), Column::i32(0, NumFormat::invariant()), Column::i32(0, $fields)] as $column) {
            $batch = DelimitedReader::fromString($data, Dialect::csv(), [$column])->read();
            $this->assertEquals(new Success(1_234_567), $batch->get(0, 0));
            $this->assertSame(CastFailure::Malformed, $batch->get(0, 1)->reason);
        }
    }

    public function testNeitherPolicyReadsWhatNeitherWrites(): void
    {
        foreach ([NumFormat::invariant(), self::indian()] as $format) {
            $batch = DelimitedReader::fromString(
                "a\n\"12,34\"\n\"1,2,345\"\n",
                Dialect::csv(),
                [Column::i32(0, $format)]
            )
                ->read();
            $this->assertSame([null, null], $batch->values(0));
            $this->assertSame(CastFailure::Malformed, $batch->get(0, 0)->reason);
            $this->assertSame(CastFailure::Malformed, $batch->get(0, 1)->reason);
        }
    }
}
