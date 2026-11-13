<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperCast\CastFailure;
use HyperCast\Checked;
use HyperCast\Constraints;
use HyperCast\Decimal;
use HyperCast\Door as CastDoor;
use HyperCast\Fault;
use HyperCast\Keyword;
use HyperCast\Rounding;
use HyperCast\Success;
use HyperCast\Violation;
use HyperTabular\Batch;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Door;
use HyperTabular\Row;
use HyperTabular\SheetOptions;
use HyperTabular\Workbook;
use PHPUnit\Framework\TestCase;

/**
 * A column that declares HyperCast's constraints: what the corpus does not reach of the binding's
 * surface. A PHP column can be handed any declaration, so the pairing with its door is checked when
 * the column is made and again when the plan is bound.
 */
final class ConstraintsTest extends TestCase
{
    private static function headerless(): Dialect
    {
        return new Dialect(',', hasHeader: false);
    }

    private static function read(string $csv, Column ...$plan): Batch
    {
        $batch = DelimitedReader::fromString($csv, self::headerless(), $plan)->read();
        self::assertNotNull($batch);
        return $batch;
    }

    private static function maximumOf(int $maximum): Constraints
    {
        return new Constraints(CastDoor::I32, maximum: $maximum);
    }

    /** The row at `$index` of `$batch`. */
    private static function row(Batch $batch, int $index): Row
    {
        foreach ($batch as $at => $row) {
            if ($at === $index) {
                return $row;
            }
        }
        self::fail("no row {$index}");
    }

    /**
     * A Column built without its factory, as a caller could: `$door` declaring `$constraints`.
     * PHP 8.3 can't write an internal class's readonly slot from outside
     * (`setRawValueWithoutLazyInitialization` is PHP 8.4's), so there no such column can be made.
     */
    private static function smuggled(Door $door, Constraints $constraints): Column
    {
        if (!method_exists(\ReflectionProperty::class, 'setRawValueWithoutLazyInitialization')) {
            self::markTestSkipped('PHP 8.3 cannot forge a column: none but a factory\'s reaches a plan there');
        }
        $source = Column::i32(0);
        $column = (new \ReflectionClass(Column::class))->newInstanceWithoutConstructor();
        foreach ((new \ReflectionClass(Column::class))->getProperties() as $slot) {
            $slot->setRawValueWithoutLazyInitialization($column, match ($slot->getName()) {
                'door' => $door,
                'constraints' => $constraints,
                default => $source->{$slot->getName()},
            });
        }
        return $column;
    }

    public function testAViolationKeepsTheValueAsCast(): void
    {
        $batch = self::read("11\n7\nx\n", Column::i32(0, constraints: self::maximumOf(10)));
        self::assertEquals(self::maximumOf(10), $batch->columns()[0]->constraints);

        self::assertEquals(new Violation(Keyword::Maximum, 11, 0, 2), $batch->getChecked(0, 0));
        self::assertSame([11, 7, null], $batch->values(0), 'the value as cast, kept');
        self::assertSame([2], array_keys($batch->faults(0)), "a violation cast, so it isn't a fault");
        self::assertEquals(new Checked(7, false, null), $batch->getChecked(0, 1));
        $fault = $batch->getChecked(0, 2);
        self::assertInstanceOf(Fault::class, $fault);
        self::assertSame(CastFailure::Malformed, $fault->reason);
        self::assertEquals($batch->getChecked(0, 1), self::row($batch, 1)->getChecked(0));
    }

    public function testARoundedValueIsFlaggedAdjusted(): void
    {
        $declared = new Constraints(CastDoor::Decimal, multipleOf: '0.5', rounding: Rounding::Floor);
        $batch = self::read("1.3\n1.5\n", Column::decimal(0, constraints: $declared));
        self::assertEquals(new Checked(new Decimal('1', 0, false), true, null), $batch->getChecked(0, 0));
        self::assertEquals(new Checked(new Decimal('15', 1, false), false, null), $batch->getChecked(0, 1));
    }

    public function testATextEnumMatchDeliversTheDeclaredEntry(): void
    {
        $declared = new Constraints(CastDoor::Text, enum: ['Female', 'Male']);
        $batch = self::read("MALE\nother\n", Column::text(0, $declared));
        self::assertEquals(new Checked('Male', false, 1), $batch->getChecked(0, 0));
        self::assertSame(['MALE', 'other'], $batch->values(0), 'the text as cast');
        self::assertEquals(new Violation(Keyword::Enum, 'other', 0, 5), $batch->getChecked(0, 1));
    }

    public function testAnUnconstrainedColumnKeepsItsFaultsUnderTheCheckedGetter(): void
    {
        $batch = self::read("5\n\"\"\nx\n", Column::i32(0));
        self::assertNull($batch->columns()[0]->constraints);
        self::assertEquals(new Checked(5, false, null), $batch->getChecked(0, 0));
        self::assertEquals($batch->get(0, 1), $batch->getChecked(0, 1));
        self::assertSame(CastFailure::Empty, $batch->getChecked(0, 1)->reason);
        self::assertEquals($batch->get(0, 2), $batch->getChecked(0, 2));
        self::assertSame(CastFailure::Malformed, $batch->getChecked(0, 2)->reason);
    }

    public function testAWholeColumnReadsAsCheckedVerdicts(): void
    {
        $batch = self::read("11\n7\nx\n", Column::i32(0, constraints: self::maximumOf(10)), Column::i32(0));
        foreach ([0, 1] as $column) {
            self::assertEquals(
                array_map(static fn (int $row) => $batch->getChecked($column, $row), [0, 1, 2]),
                $batch->checkedVerdicts($column),
                "column {$column}"
            );
        }
        self::assertSame(
            [Violation::class, Checked::class, Fault::class],
            array_map(static fn (object $verdict) => $verdict::class, $batch->checkedVerdicts(0))
        );
        self::assertEquals(new Checked(11, false, null), $batch->checkedVerdicts(1)[0]);
        $this->expectException(\OutOfRangeException::class);
        $batch->checkedVerdicts(2);
    }

    public function testThePlainGettersRefuseAConstrainedColumn(): void
    {
        $batch = self::read(
            "7,a\n",
            Column::i32(0, constraints: self::maximumOf(10)),
            Column::text(1, new Constraints(CastDoor::Text, maxLength: 3)),
            Column::i32(0),
        );
        $refusals = [
            'get' => [0, static fn () => $batch->get(0, 0)],
            'verdicts' => [1, static fn () => $batch->verdicts(1)],
            'row get' => [0, static fn () => self::row($batch, 0)->get(0)],
            'row get, text' => [1, static fn () => self::row($batch, 0)->get(1)],
        ];
        foreach ($refusals as $name => [$column, $read]) {
            try {
                $read();
                self::fail("{$name} read a constrained column");
            } catch (\LogicException $refused) {
                self::assertSame(
                    "Column {$column} declares constraints: read it with getChecked()",
                    $refused->getMessage(),
                    $name
                );
            }
        }
        // The control: the checked getter reads them, the unconstrained column reads as ever, and
        // the whole-column views are there.
        self::assertEquals(new Checked(7, false, null), $batch->getChecked(0, 0));
        self::assertEquals(new Checked('a', false, null), $batch->getChecked(1, 0));
        self::assertEquals(new Success(7), $batch->get(2, 0));
        self::assertSame([7], $batch->values(0));
        self::assertSame([], $batch->faults(1));
    }

    public function testAMismatchedDoorIsRefused(): void
    {
        $refused = static function (callable $make, string $message): void {
            try {
                $make();
                self::fail("nothing refused: {$message}");
            } catch (\InvalidArgumentException $error) {
                self::assertSame($message, $error->getMessage());
            }
        };
        $refused(
            static fn () => Column::u8(0, constraints: self::maximumOf(10)),
            'Constraints for Door::I32 were given to a Door::U8 column',
        );
        // A door that takes no constraints has none to take.
        $refused(
            static fn () => DelimitedReader::fromString(
                "1\n",
                self::headerless(),
                [self::smuggled(Door::Uuid, new Constraints(CastDoor::Text))],
            ),
            'Plan column 0: Constraints for Door::Text were given to a Door::Uuid column',
        );
        // A column that got past its factory is refused when its plan is bound, on a reader made
        // with the plan and on one bound to it after its header.
        $refused(
            static fn () => DelimitedReader::fromString(
                "1\n",
                self::headerless(),
                [self::smuggled(Door::I64, self::maximumOf(10))],
            ),
            'Plan column 0: Constraints for Door::I32 were given to a Door::I64 column',
        );
        $reader = DelimitedReader::fromString("1\n", self::headerless());
        $refused(
            static fn () => $reader->bind([self::smuggled(Door::I64, self::maximumOf(10))]),
            'Plan column 0: Constraints for Door::I32 were given to a Door::I64 column',
        );
        // The control: the same column, rightly paired, is bound.
        $plan = [self::smuggled(Door::I32, self::maximumOf(10))];
        $batch = DelimitedReader::fromString("1\n", self::headerless(), $plan)->read();
        self::assertNotNull($batch);
        self::assertEquals(new Checked(1, false, null), $batch->getChecked(0, 0));
    }

    public function testAnUnbuiltDeclarationIsRefused(): void
    {
        $unbuilt = (new \ReflectionClass(Constraints::class))->newInstanceWithoutConstructor();
        // PHP's own refusal of an object its constructor never built.
        $this->expectException(\Error::class);
        $this->expectExceptionMessage('Constraints::$door must not be accessed before initialization');
        Column::i32(0, constraints: $unbuilt);
    }

    public function testFaultsListAViolationWithItsKeyword(): void
    {
        $listed = self::read("11\nx\n", Column::i32(0, constraints: self::maximumOf(10)))->faults();
        self::assertCount(2, $listed);
        self::assertSame(
            [Keyword::Maximum, Keyword::Maximum->value, null, 0, 2],
            [$listed[0]->keyword, $listed[0]->keywordCode, $listed[0]->reason, $listed[0]->offset, $listed[0]->length],
        );
        self::assertSame(
            [null, 0, CastFailure::Malformed],
            [$listed[1]->keyword, $listed[1]->keywordCode, $listed[1]->reason],
        );
    }

    public function testChecksAreSizedLikeVerdicts(): void
    {
        $batch = self::read(str_repeat("11\n", 37), Column::i32(0, constraints: self::maximumOf(10)));
        self::assertSame(37, $batch->rows());
        self::assertEquals(new Violation(Keyword::Maximum, 11, 0, 2), $batch->getChecked(0, 36));
        self::assertSame(
            array_fill(0, 37, Keyword::Maximum),
            array_map(static fn ($problem) => $problem->keyword, $batch->faults()),
        );
    }

    public function testAConstrainedSheetColumn(): void
    {
        $book = Workbook::open(\dirname(__DIR__, 2) . '/corpus/workbook/constraints.xlsx');
        $sheet = $book->sheet(0, new SheetOptions(hasHeader: true), [Column::i32(0, constraints: self::maximumOf(10))]);
        $verdicts = [];
        while (($batch = $sheet->read()) !== null) {
            foreach ($batch as $row) {
                $verdicts[] = $row->getChecked(0);
            }
        }
        self::assertEquals(new Checked(0, false, null), $verdicts[0]);
        self::assertContainsEquals(new Violation(Keyword::Maximum, 42, 0, 0), $verdicts);
    }

    public function testAColumnKeepsItsConstraintsAndComparesBy(): void
    {
        $column = Column::i32(0, constraints: self::maximumOf(10));
        self::assertEquals(self::maximumOf(10), $column->withTokens(['NA'])->constraints, 'carried through withTokens');
        self::assertEquals(Column::i32(0, constraints: self::maximumOf(10)), $column);
        self::assertNotEquals(Column::i32(0), $column);
        self::assertNotEquals(Column::i32(0, constraints: self::maximumOf(11)), $column);
    }
}
