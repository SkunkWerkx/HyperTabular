<?php

declare(strict_types=1);

namespace HyperTabular\Tests;

use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Tests\Support\Calling;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * A stream's `fread()` runs the caller's PHP in the middle of a read, and that PHP can reach the
 * reader it feeds. Every call it makes into that reader, or into the rows iterating it, is refused
 * with a `LogicException`, and the read it interrupted goes on as if nothing had been asked.
 */
final class ReentryTest extends TestCase
{
    private const READER_BUSY = "The reader is in the middle of a read; its stream can't call back into it";
    private const ITERATOR_BUSY =
        "The row iterator is in the middle of a read; its reader's stream can't call back into it";
    private const PARTS = ["a,b\n", "1,2\n", "3,4\n", "5,6\n"];

    /** @return iterable<string, array{callable(DelimitedReader): mixed}> */
    public static function calls(): iterable
    {
        yield 'read' => [static fn (DelimitedReader $reader) => $reader->read()];
        yield 'close' => [static fn (DelimitedReader $reader) => $reader->close()];
        yield 'bind' => [static fn (DelimitedReader $reader) => $reader->bind([Column::i32(0)])];
        yield 'records' => [static fn (DelimitedReader $reader) => $reader->records()];
        yield 'columnCount' => [static fn (DelimitedReader $reader) => $reader->columnCount()];
        yield 'header' => [static fn (DelimitedReader $reader) => $reader->header()];
        yield 'headerIndex' => [static fn (DelimitedReader $reader) => $reader->headerIndex()];
        yield 'plan' => [static fn (DelimitedReader $reader) => $reader->plan()];
        yield 'isBound' => [static fn (DelimitedReader $reader) => $reader->isBound()];
        yield 'rows, then a step' => [static fn (DelimitedReader $reader) => $reader->rows()->valid()];
    }

    /**
     * @param callable(DelimitedReader): mixed $call
     * @return array{list<int>, list<string>}
     */
    private static function readCalling(callable $call): array
    {
        $held = null;
        $said = [];
        $stream = Calling::open(self::PARTS, 3, static function () use (&$held, &$said, $call): void {
            try {
                $call($held);
                $said[] = 'taken';
            } catch (\LogicException $refused) {
                $said[] = $refused->getMessage();
            }
        });
        $held = DelimitedReader::fromStream(
            $stream,
            Dialect::csv(),
            [Column::i32(0), Column::i32(1)],
            batchRows: 1,
            bufferBytes: 4
        );
        $values = [];
        while (($batch = $held->read()) !== null) {
            array_push($values, ...$batch->values(0));
        }
        $held->close();
        fclose($stream);
        return [$values, $said];
    }

    /** @param callable(DelimitedReader): mixed $call */
    #[DataProvider('calls')]
    public function testACallBackIntoTheReaderMidReadIsRefusedAndTheReadGoesOn(callable $call): void
    {
        [$values, $said] = self::readCalling($call);
        $this->assertSame([self::READER_BUSY], $said);
        $this->assertSame([1, 3, 5], $values);
    }

    public function testTheDialectIsAnsweredMidRead(): void
    {
        [$values, $said] = self::readCalling(static fn (DelimitedReader $reader) => $reader->dialect());
        $this->assertSame(['taken'], $said);
        $this->assertSame([1, 3, 5], $values);
    }

    public function testACallBackIntoTheRowsMidReadIsRefusedAndTheRowsGoOn(): void
    {
        $iterator = null;
        $said = [];
        $stream = Calling::open(self::PARTS, 3, static function () use (&$iterator, &$said): void {
            foreach (
                [
                    static fn () => $iterator->next(),
                    static fn () => $iterator->current(),
                    static fn () => $iterator->key(),
                    static fn () => $iterator->valid(),
                    static fn () => $iterator->rewind(),
                ] as $call
            ) {
                try {
                    $call();
                    $said[] = 'taken';
                } catch (\LogicException $refused) {
                    $said[] = $refused->getMessage();
                }
            }
        });
        $reader = DelimitedReader::fromStream($stream, Dialect::csv(), [Column::i32(0)], batchRows: 1, bufferBytes: 4);
        $iterator = $reader->rows();
        $values = [];
        foreach ($iterator as $key => $row) {
            $values[$key] = $row->value(0);
        }
        fclose($stream);
        $this->assertSame(array_fill(0, 5, self::ITERATOR_BUSY), $said);
        $this->assertSame([1, 3, 5], $values);
    }

    public function testAnotherReaderAndASuggestionRunMidRead(): void
    {
        $said = [];
        $stream = Calling::open(self::PARTS, 3, static function () use (&$said): void {
            $other = DelimitedReader::fromString("x\n7\n", Dialect::csv(), [Column::i32(0)]);
            $said[] = $other->read()->values(0);
            $said[] = DelimitedReader::suggestString("x\n7\n", Dialect::csv())->report[0]->sampled;
        });
        $reader = DelimitedReader::fromStream($stream, Dialect::csv(), [Column::i32(0)], batchRows: 1, bufferBytes: 4);
        $values = [];
        while (($batch = $reader->read()) !== null) {
            array_push($values, ...$batch->values(0));
        }
        fclose($stream);
        $this->assertSame([[7], 1], $said);
        $this->assertSame([1, 3, 5], $values);
    }
}
