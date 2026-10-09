<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;
use FFI\CData;

/**
 * A forward-only read of one sheet of a {@see Workbook}, a batch at a time, through a plan —
 * into the same {@see Batch} delimited text is read into.
 *
 * A typed cell is converted directly by its door — a stored `42.0` never passes through
 * text to become an int — and a text cell goes through the door as delimited text would. A
 * sheet that is structurally broken throws a {@see TabularException} after every intact row
 * before the break has been delivered, and the same one again on every later read.
 *
 * A sheet opened without a plan reads its header row and stops: {@see headerIndex()} looks
 * each name up, and {@see bind()} declares the plan, before the first read.
 */
final class Sheet
{
    /** The plan, as the core takes it: null until one is bound. */
    private ?Columns $columns = null;
    private CData $state;
    private Scratch $scratch;
    /** Cell-table entries one row takes: one per plan column, and one for the row's number. */
    private int $perRow = 0;
    /** @var list<string>|null */
    private ?array $header = null;
    private ?Header $headerIndex = null;
    /** A failure met with rows before it: those went out first, and this is next. */
    private ?TabularException $pending = null;
    private ?TabularException $failure = null;

    /**
     * Made by {@see Workbook::sheet()}: binds the plan if there is one, positions a copy of
     * the workbook's state on the sheet, and reads its header if the options declare one.
     *
     * @param Workbook $book the workbook
     * @param SheetInfo $info the sheet
     * @param SheetOptions $options how it is read
     * @param array<mixed>|null $plan the output columns, in output order, or null for none yet
     * @throws \InvalidArgumentException when the plan cannot be honoured
     * @throws TabularException when the sheet, or its header row, is structurally broken
     * @internal
     */
    public function __construct(
        private readonly Workbook $book,
        SheetInfo $info,
        private readonly SheetOptions $options,
        ?array $plan,
    ) {
        $this->state = $book->copyOfState();
        // With a plan, binding it sizes the cell table and the row; without one, the header
        // row is read into a cell table of its own size and a row of at least as many slots
        // (see readHeader()), which a plan bound later grows and never shrinks.
        if ($plan === null) {
            $this->scratch = Scratch::$stingy ? new Scratch(1, 1, 1, 1) : new Scratch(0, 4096, 64, 64);
        } else {
            $this->scratch = Scratch::$stingy ? new Scratch(1, 1, 1, 0) : new Scratch(0, 4096, 0, 0);
            $this->bind($plan);
        }
        [$container, $length] = $book->container();
        $code = Native::ffi()->hypertabular_workbook_sheet(
            $this->state,
            $container,
            $length,
            $info->part,
            \strlen($info->part),
            $info->index,
            $options->hasHeader ? 1 : 0,
            $options->skipEmptyRows ? 1 : 0,
            FFI::addr($this->scratch->filled)
        );
        Scratch::settle($code, $this->scratch->filled);
        if ($options->hasHeader) {
            $this->readHeader();
        }
    }

    /**
     * The options the sheet is read with.
     *
     * @return SheetOptions the options
     */
    public function options(): SheetOptions
    {
        return $this->options;
    }

    /**
     * Declares the plan a sheet opened without one reads through — once, before the first
     * read: {@see DelimitedReader::bind()}, for a sheet. A plan that is refused leaves the
     * sheet as it was, unbound.
     *
     * @param list<Column> $plan the output columns, in output order
     * @return void
     * @throws \LogicException when the sheet already has a plan
     * @throws \InvalidArgumentException when the plan cannot be honoured
     */
    public function bind(array $plan): void
    {
        if ($this->columns !== null) {
            throw new \LogicException('The sheet already has a plan; a plan is bound once');
        }
        $columns = new Columns($plan, $this->options->batchRows);
        $this->perRow = $columns->count + 1;
        // The cell table takes a batch's rows up front — unless the tests asked every buffer
        // to start with no room, when the fill asks for it instead.
        if (!Scratch::$stingy) {
            $this->scratch->growCellsTo($columns->batchRows * $this->perRow);
        }
        // A slot for every source column the plan reaches. What the slots already hold — a
        // header row the sheet may yet repeat — is kept.
        $this->scratch->growRowTo($columns->width);
        $this->columns = $columns;
    }

    /**
     * Whether a plan has been bound: always, for a sheet opened with one.
     *
     * @return bool whether {@see read()} can be called
     */
    public function isBound(): bool
    {
        return $this->columns !== null;
    }

    /**
     * The plan the sheet is read through: column `i` of every batch is `plan()[i]`. Empty
     * until one is bound.
     *
     * @return list<Column> the output columns, in output order
     */
    public function plan(): array
    {
        return $this->columns?->plan ?? [];
    }

    /**
     * The header row's names — a typed cell said the way the text door says it — or null
     * when the options declare no header.
     *
     * @return list<string>|null the names
     */
    public function header(): ?array
    {
        return $this->header;
    }

    /**
     * The header as a {@see Header}: the same names, and the lookup by name a plan is built
     * with. Null when the options declare no header.
     *
     * @return Header|null the header
     */
    public function headerIndex(): ?Header
    {
        if ($this->header === null) {
            return null;
        }
        return $this->headerIndex ??= new Header($this->header);
    }

    /**
     * Every row left, batch by batch: {@see DelimitedReader::rows()}, for a sheet.
     *
     * @return \Generator<int, Row> the rows, keyed from zero across the whole read
     * @throws TabularException when the sheet is structurally broken, after every intact row
     *     before the break
     * @throws \LogicException when the sheet has no plan yet
     */
    public function rows(): \Generator
    {
        while (($batch = $this->read()) !== null) {
            foreach ($batch as $row) {
                yield $row;
            }
        }
    }

    /**
     * Reads the next batch: up to the options' batch size of rows, or null once the sheet
     * has no more.
     *
     * @return Batch|null the rows, or null at the end
     * @throws TabularException when the sheet is structurally broken — thrown after every
     *     intact row before the break has been delivered, and again on every later call
     * @throws \LogicException when the sheet has no plan yet ({@see bind()}) — which a later
     *     call, once one is bound, does not repeat
     */
    public function read(): ?Batch
    {
        $columns = $this->columns ?? throw new \LogicException(
            'The sheet has no plan to read through; bind() one first'
        );
        if ($this->pending !== null) {
            $this->failure = $this->pending;
            $this->pending = null;
        }
        if ($this->failure !== null) {
            throw $this->failure;
        }
        $ffi = Native::ffi();
        $scratch = $this->scratch;
        [$container, $length] = $this->book->container();
        $code = $scratch->drive(fn (CData $buffers): int => $ffi->hypertabular_workbook_fill(
            $this->state,
            $container,
            $length,
            $columns->specs,
            $columns->buffers,
            $columns->count,
            $columns->batchRows,
            $buffers,
            FFI::addr($scratch->filled)
        ), $this->book);
        $rows = $scratch->filled->rows;
        if ($code !== Native::OK) {
            try {
                Scratch::settle($code, $scratch->filled);
            } catch (TabularException $broken) {
                if ($rows === 0) {
                    throw $this->failure = $broken;
                }
                $this->pending = $broken;
            }
        }
        if ($rows === 0) {
            return null;
        }
        // The arena as far as the core wrote into it, which is as far as any of the batch's
        // spans reach — the text typed cells were said as, and the raw text of those that
        // failed.
        $arenaUsed = $scratch->filled->arena_used;
        return new Batch(
            $columns->plan,
            $rows,
            $columns->values($rows),
            $columns->verdicts($rows),
            FFI::string($scratch->cells, $rows * $this->perRow * 8),
            $this->perRow,
            $this->book->strings(),
            $arenaUsed > 0 ? FFI::string($scratch->arena, $arenaUsed) : '',
            true
        );
    }

    /**
     * Reads the header row: a span per name — in the shared strings, or flagged, in the
     * arena.
     *
     * @return void
     */
    private function readHeader(): void
    {
        $ffi = Native::ffi();
        $scratch = $this->scratch;
        // The header row's cells are kept in the row's slots as well as named — the slots
        // are what a row the sheet repeats (an ODS `number-rows-repeated`) is delivered
        // again from. A plan says how many slots it reads; without one, there is a slot for
        // every name the cell table can hold, the two grown together, and binding a plan
        // later keeps them.
        $unbound = $this->columns === null;
        if ($unbound) {
            $scratch->growRowTo($scratch->cellsCap);
        }
        [$container, $length] = $this->book->container();
        $code = $scratch->drive(fn (CData $buffers): int => $ffi->hypertabular_workbook_header(
            $this->state,
            $container,
            $length,
            $buffers,
            FFI::addr($scratch->filled)
        ), $this->book, $unbound);
        Scratch::settle($code, $scratch->filled);
        $strings = $this->book->strings();
        $arena = FFI::string($scratch->arena, $scratch->arenaCap);
        $header = [];
        for ($index = 0; $index < $scratch->filled->rows; $index++) {
            $name = $scratch->cells[$index];
            $header[] = ($name->len & Native::SPAN_FLAG) !== 0
                ? substr($arena, $name->offset, $name->len & Native::SPAN_LENGTH)
                : substr($strings, $name->offset, $name->len);
        }
        $this->header = $header;
    }
}
