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
 */
final class Sheet
{
    private Columns $columns;
    private CData $state;
    private Scratch $scratch;
    /** Cell-table entries one row takes: one per plan column, and one for the row's number. */
    private int $perRow;
    /** @var list<string>|null */
    private ?array $header = null;
    /** A failure met with rows before it: those went out first, and this is next. */
    private ?TabularException $pending = null;
    private ?TabularException $failure = null;

    /**
     * Made by {@see Workbook::sheet()}: positions a copy of the workbook's state on the
     * sheet, and reads its header if the options declare one.
     *
     * @param Workbook $book the workbook
     * @param SheetInfo $info the sheet
     * @param SheetOptions $options how it is read
     * @param array<mixed> $plan the output columns, in output order
     * @throws \InvalidArgumentException when the plan cannot be honoured
     * @throws TabularException when the sheet, or its header row, is structurally broken
     * @internal
     */
    public function __construct(
        private readonly Workbook $book,
        SheetInfo $info,
        private readonly SheetOptions $options,
        array $plan,
    ) {
        $this->columns = new Columns($plan, $options->batchRows);
        $this->perRow = $this->columns->count + 1;
        $this->state = $book->copyOfState();
        $this->scratch = Scratch::$stingy
            ? new Scratch(1, 1, 1, $this->columns->width)
            : new Scratch(0, 4096, $options->batchRows * $this->perRow, $this->columns->width);
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
     * The plan the sheet is read through: column `i` of every batch is `plan()[i]`.
     *
     * @return list<Column> the output columns, in output order
     */
    public function plan(): array
    {
        return $this->columns->plan;
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
     * Reads the next batch: up to the options' batch size of rows, or null once the sheet
     * has no more.
     *
     * @return Batch|null the rows, or null at the end
     * @throws TabularException when the sheet is structurally broken — thrown after every
     *     intact row before the break has been delivered, and again on every later call
     */
    public function read(): ?Batch
    {
        if ($this->pending !== null) {
            $this->failure = $this->pending;
            $this->pending = null;
        }
        if ($this->failure !== null) {
            throw $this->failure;
        }
        $ffi = Native::ffi();
        $columns = $this->columns;
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
        [$container, $length] = $this->book->container();
        $code = $scratch->drive(fn (CData $buffers): int => $ffi->hypertabular_workbook_header(
            $this->state,
            $container,
            $length,
            $buffers,
            FFI::addr($scratch->filled)
        ), $this->book);
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
