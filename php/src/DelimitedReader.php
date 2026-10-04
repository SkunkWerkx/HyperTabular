<?php

declare(strict_types=1);

namespace HyperTabular;

use DateTimeImmutable;
use FFI;
use FFI\CData;
use HyperCast\CastFailure;
use HyperCast\Decimal;
use HyperCast\Duration;
use HyperCast\Fault;
use HyperCast\Success;

/**
 * Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
 * typed columns, every cell a HyperCast `Success|Fault` verdict.
 *
 * The native core (libhypertabular) owns no memory and reads no files. It is handed a
 * chunk of input and the buffers to fill, casts each plan column in one native loop, and
 * says how many rows it wrote and how many bytes it is finished with. Everything else is
 * here, which is the point of the design: this class allocates the input buffer, one value
 * array and one verdict array per column, the table that locates each cell, and the arena
 * for the rare escaped cell — once, as FFI memory, reused for every batch — and puts what
 * the core did not consume back in front of it. The native boundary is crossed once per
 * batch, not once per cell; a column is then lifted into PHP in one piece, the first time
 * it is asked for.
 *
 * A value that does not cast is that cell's verdict, and the read goes on. Input that is
 * not rows of cells at all — a record of the wrong width, a quote never closed — is a
 * {@see TabularException}, thrown after every intact row before it has been delivered.
 *
 * HyperCast is the judge: a verdict is HyperCast's own `Success` or `Fault`, a fault's
 * reason its `CastFailure`, and a value the same PHP carrier HyperCast's `Cast` returns
 * for that door ({@see Door} lists them).
 *
 * What is handed out for a batch is valid until the next {@see read()}. Not safe to share
 * between threads or fibers that read concurrently.
 */
final class DelimitedReader
{
    /** The row ceiling: a single record larger than this is a {@see TabularFailure::RowTooLong}. */
    public const MAX_ROW_BYTES = 1 << 30;

    /** The initial input buffer; it doubles when a record does not fit. */
    public const DEFAULT_BUFFER_BYTES = 256 * 1024;

    /** Rows per batch unless told otherwise. */
    public const DEFAULT_BATCH_ROWS = 4096;

    private const CONTRACT_VIOLATION =
        'hypertabular: libhypertabular reported a contract violation — a binding bug, please report it';

    private static ?bool $fastInstants = null;

    private FFI $ffi;

    /** @var list<Column> */
    private array $plan;
    private int $columns;
    private int $batchRows;
    /** Cell-table entries one row takes: the widest ordinal the plan reads, plus two. */
    private int $perRow;

    // Everything the core is handed, allocated once. A CData that owns memory is kept
    // beside every pointer into it: the pointer does not keep the memory alive.
    private CData $state;
    private CData $statePtr;
    private CData $filled;
    private CData $filledPtr;
    private CData $specs;
    private CData $buffers;
    /** @var list<CData> */
    private array $valueBuffers = [];
    /** @var list<CData> */
    private array $verdictBuffers = [];
    private CData $cells;
    private int $cellsCap;
    private CData $arena;
    private int $arenaCap;
    private ?CData $scratch = null;
    private int $scratchCap = 0;

    // The source: a stream, or a string handed over a chunk at a time.
    /** @var resource|null */
    private $stream = null;
    private bool $ownsStream = false;
    private ?string $text = null;
    private int $textAt = 0;

    // The input buffer: the bytes at [start, end) are what the core has not finished with.
    private CData $buffer;
    private CData $bufferPtr;
    private int $capacity;
    private int $start = 0;
    private int $end = 0;
    private bool $eof = false;

    // The batch in hand, and the window of input its spans point into.
    private int $rows = 0;
    private int $windowAt = 0;
    private int $windowBytes = 0;
    private int $arenaUsed = 0;
    private ?string $window = null;
    private ?string $arenaBytes = null;
    /** @var array<int, list<mixed>> each decoded column's values, null where the cell did not cast */
    private array $values = [];
    /** @var array<int, array<int, Fault>> each decoded column's faults, by row */
    private array $faults = [];

    /** @var list<string>|null */
    private ?array $header = null;
    private ?TabularException $failure = null;
    private bool $closed = false;

    /**
     * Reads UTF-8 delimited text already in memory. The string is handed to the core
     * through the reader's own buffer, so a string no larger than `$bufferBytes` is read
     * whole, in one piece, and a larger one a buffer at a time.
     *
     * @param string $utf8 the text
     * @param Dialect $dialect the declared dialect
     * @param list<Column> $plan the output columns, in output order
     * @param int $batchRows rows per batch
     * @param int $bufferBytes the initial input buffer; it doubles when a record does not fit
     * @return self the reader, its header (if the dialect declares one) already read
     * @throws \InvalidArgumentException when the dialect's separator or the plan cannot be honoured
     * @throws TabularException when the header record is structurally broken
     */
    public static function fromString(
        string $utf8,
        Dialect $dialect,
        array $plan,
        int $batchRows = self::DEFAULT_BATCH_ROWS,
        int $bufferBytes = self::DEFAULT_BUFFER_BYTES,
    ): self {
        if ($bufferBytes < 1) {
            throw new \InvalidArgumentException("The buffer must hold at least one byte; got {$bufferBytes}");
        }
        $buffer = max(1, min(\strlen($utf8), $bufferBytes, self::MAX_ROW_BYTES));
        $reader = new self($dialect, $plan, $batchRows, $buffer);
        $reader->text = $utf8;
        $reader->eof = $utf8 === '';
        $reader->begin($dialect);
        return $reader;
    }

    /**
     * Reads UTF-8 delimited text from a stream — read forward only, never sought. The
     * stream stays the caller's: {@see close()} does not close it. It has to block until it
     * has data or has ended; a read that returns nothing before the end (a non-blocking
     * stream, a timeout) is an error, never taken for the end of the input.
     *
     * @param resource $stream a readable stream
     * @param Dialect $dialect the declared dialect
     * @param list<Column> $plan the output columns, in output order
     * @param int $batchRows rows per batch
     * @param int $bufferBytes the initial input buffer; it doubles when a record does not fit
     * @return self the reader, its header (if the dialect declares one) already read
     * @throws \InvalidArgumentException when `$stream` is not a stream, or the dialect's
     *     separator or the plan cannot be honoured
     * @throws TabularException when the header record is structurally broken
     */
    public static function fromStream(
        $stream,
        Dialect $dialect,
        array $plan,
        int $batchRows = self::DEFAULT_BATCH_ROWS,
        int $bufferBytes = self::DEFAULT_BUFFER_BYTES,
    ): self {
        if (!\is_resource($stream) || get_resource_type($stream) !== 'stream') {
            throw new \InvalidArgumentException('The stream must be an open stream resource');
        }
        if ($bufferBytes < 1) {
            throw new \InvalidArgumentException("The buffer must hold at least one byte; got {$bufferBytes}");
        }
        $reader = new self($dialect, $plan, $batchRows, min($bufferBytes, self::MAX_ROW_BYTES));
        $reader->stream = $stream;
        $reader->begin($dialect);
        return $reader;
    }

    /**
     * Opens a file of UTF-8 delimited text. The reader owns the file handle and closes it
     * on {@see close()}, or when it is destroyed.
     *
     * @param string $path the file
     * @param Dialect $dialect the declared dialect
     * @param list<Column> $plan the output columns, in output order
     * @param int $batchRows rows per batch
     * @param int $bufferBytes the initial input buffer; it doubles when a record does not fit
     * @return self the reader, its header (if the dialect declares one) already read
     * @throws \InvalidArgumentException when the dialect's separator or the plan cannot be honoured
     * @throws \RuntimeException when the file cannot be opened
     * @throws TabularException when the header record is structurally broken
     */
    public static function open(
        string $path,
        Dialect $dialect,
        array $plan,
        int $batchRows = self::DEFAULT_BATCH_ROWS,
        int $bufferBytes = self::DEFAULT_BUFFER_BYTES,
    ): self {
        $stream = @fopen($path, 'rb');
        if ($stream === false) {
            $reason = error_get_last()['message'] ?? 'unknown error';
            throw new \RuntimeException("hypertabular: cannot open {$path}: {$reason}");
        }
        try {
            $reader = self::fromStream($stream, $dialect, $plan, $batchRows, $bufferBytes);
        } catch (\Throwable $error) {
            fclose($stream);
            throw $error;
        }
        $reader->ownsStream = true;
        return $reader;
    }

    /**
     * Allocates everything the core will be handed, and starts its state for the dialect.
     *
     * @param Dialect $dialect the declared dialect
     * @param array<mixed> $plan the output columns, in output order
     * @param int $batchRows rows per batch
     * @param int $bufferBytes the initial input buffer
     * @throws \InvalidArgumentException when the dialect's separator or the plan cannot be honoured
     */
    private function __construct(Dialect $dialect, array $plan, int $batchRows, int $bufferBytes)
    {
        if ($batchRows < 1) {
            throw new \InvalidArgumentException("A batch must hold at least one row; got {$batchRows}");
        }
        $plan = array_values($plan);
        foreach ($plan as $index => $column) {
            if (!$column instanceof Column) {
                throw new \InvalidArgumentException(
                    "Plan entry {$index} is not a Column; build one with Column's factories"
                );
            }
        }
        $ffi = $this->ffi = Native::ffi();
        $this->plan = $plan;
        $this->columns = \count($plan);
        $this->batchRows = $batchRows;

        // FFI cannot allocate nothing, and a plan may be empty (rows are still counted).
        $slots = max(1, $this->columns);
        $this->specs = $ffi->new("ht_column_spec[{$slots}]");
        $this->buffers = $ffi->new("ht_column_buffer[{$slots}]");
        $widest = -1;
        foreach ($plan as $index => $column) {
            $widest = max($widest, $column->ordinal);
            $spec = $this->specs[$index];
            $spec->ordinal = $column->ordinal;
            $spec->door = $column->door->value;
            $spec->param = $column->declared;
            // HyperCast's NumFormat in the core's 32-byte layout. The object validated itself
            // when it was built, as HyperCast validates it, so the core has nothing to refuse.
            [$decimal, $group] = $column->format->codePoints();
            $spec->format->decimal_sep = $decimal;
            $spec->format->group_sep = $group;
            $spec->format->flags = $column->format->flags;
            $currency = $column->format->currency;
            $spec->format->currency_len = \strlen($currency);
            if ($currency !== '') {
                FFI::memcpy($spec->format->currency, $currency, \strlen($currency));
            }

            $values = $ffi->new('uint8_t[' . $batchRows * $column->door->valueSize() . ']');
            $verdicts = $ffi->new("ht_verdict[{$batchRows}]");
            $this->valueBuffers[] = $values;
            $this->verdictBuffers[] = $verdicts;
            $this->buffers[$index]->values = FFI::addr($values);
            $this->buffers[$index]->verdicts = $ffi->cast('ht_verdict *', FFI::addr($verdicts));
        }
        $this->perRow = $widest + 2;
        $this->cellsCap = $this->perRow * $batchRows;
        $this->cells = $ffi->new("ht_span[{$this->cellsCap}]");
        $this->arenaCap = 4096;
        $this->arena = $ffi->new("uint8_t[{$this->arenaCap}]");
        $this->capacity = $bufferBytes;
        $this->buffer = $ffi->new("uint8_t[{$bufferBytes}]");
        $this->bufferPtr = $ffi->cast('uint8_t *', FFI::addr($this->buffer));
        $this->filled = $ffi->new('ht_filled');
        $this->filledPtr = FFI::addr($this->filled);

        $this->state = $ffi->new('ht_state');
        $this->statePtr = FFI::addr($this->state);
        $raw = $ffi->new('ht_dialect');
        $raw->separator = \ord($dialect->separator);
        $raw->quoting = $dialect->quoting ? 1 : 0;
        $raw->skip_blank_lines = $dialect->skipBlankLines ? 1 : 0;
        if ($ffi->hypertabular_delimited_init($this->statePtr, FFI::addr($raw)) !== Native::OK) {
            throw new \InvalidArgumentException(sprintf(
                'Separator 0x%02X is not tab or printable ASCII other than \'"\'',
                \ord($dialect->separator)
            ));
        }
        self::$fastInstants ??= method_exists(DateTimeImmutable::class, 'createFromTimestamp')
            && method_exists(DateTimeImmutable::class, 'setMicrosecond');
    }

    /**
     * Releases the file handle of a reader that opened one, if {@see close()} was not called.
     */
    public function __destruct()
    {
        $this->close();
    }

    /**
     * The header's names, when the dialect declares one: null when it declares none, and
     * empty for an input with no record at all.
     *
     * @return list<string>|null the names, in the order the input wrote them
     */
    public function header(): ?array
    {
        return $this->header;
    }

    /**
     * The plan the reader was built with.
     *
     * @return list<Column> the output columns, in output order
     */
    public function plan(): array
    {
        return $this->plan;
    }

    /**
     * The number of plan columns.
     *
     * @return int the plan's length
     */
    public function columnCount(): int
    {
        return $this->columns;
    }

    /**
     * Rows in the batch in hand: 0 before the first {@see read()} and after the last.
     *
     * @return int the number of rows every column of the batch holds
     */
    public function rows(): int
    {
        return $this->rows;
    }

    /**
     * Records finished so far — the header and skipped blank lines included.
     *
     * @return int the count, as the core keeps it
     */
    public function records(): int
    {
        return $this->state->records;
    }

    /**
     * Reads the next batch: one native call fills every column. True with {@see rows()}
     * rows in hand; false once the input is exhausted.
     *
     * @return bool whether a batch is in hand
     * @throws TabularException when the input is structurally broken — thrown after every
     *     intact row before the break has been delivered, and again on every later call
     * @throws \LogicException when the reader has been closed
     * @throws \RuntimeException when the stream cannot be read
     */
    public function read(): bool
    {
        if ($this->closed) {
            throw new \LogicException('The reader is closed');
        }
        if ($this->failure !== null) {
            throw $this->failure;
        }
        $this->rows = 0;
        $this->window = null;
        $this->arenaBytes = null;
        $this->values = [];
        $this->faults = [];
        $ffi = $this->ffi;
        $filled = $this->filled;
        while (true) {
            $length = $this->end - $this->start;
            $last = $this->eof;
            $code = $ffi->hypertabular_delimited_fill(
                $this->statePtr,
                $this->bufferPtr + $this->start,
                $length,
                $last ? 1 : 0,
                $this->specs,
                $this->buffers,
                $this->columns,
                $this->batchRows,
                $this->cells,
                $this->cellsCap,
                $this->arena,
                $this->arenaCap,
                $this->filledPtr
            );
            switch ($code) {
                case Native::OK:
                    $consumed = $filled->consumed;
                    if ($filled->rows > 0) {
                        $this->rows = $filled->rows;
                        $this->windowAt = $this->start;
                        $this->windowBytes = $consumed;
                        $this->arenaUsed = $filled->arena_used;
                        $this->start += $consumed;
                        return true;
                    }
                    $this->start += $consumed;
                    if ($last && ($consumed === $length || $consumed === 0)) {
                        return false;
                    }
                    if ($consumed === 0) {
                        $this->refill();
                    }
                    break;
                case Native::ERR_CELLS:
                    $this->cellsCap = $filled->needed * $this->batchRows;
                    $this->cells = $ffi->new("ht_span[{$this->cellsCap}]");
                    break;
                case Native::ERR_ARENA:
                    $this->growArena($filled->needed);
                    break;
                case Native::ERR_STRUCTURE:
                    throw $this->structural();
                default:
                    throw new \RuntimeException(self::CONTRACT_VIOLATION);
            }
        }
    }

    /**
     * A column of the batch in hand, one value per row, each the PHP carrier of the
     * column's door ({@see Door}) — and null where the cell did not cast, whatever the
     * reason; {@see faults()} says why. The column is lifted out of the core's buffer in one
     * piece the first time it is asked for, and kept until the next {@see read()}.
     *
     * @param int $column the plan column
     * @return list<mixed> the values, by row
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function values(int $column): array
    {
        $this->decode($column);
        return $this->values[$column];
    }

    /**
     * The cells of a column of the batch in hand that did not cast: HyperCast's `Fault`
     * for each, keyed by row. Empty when every cell cast.
     *
     * @param int $column the plan column
     * @return array<int, Fault> the faults, by row, in row order
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function faults(int $column): array
    {
        $this->decode($column);
        return $this->faults[$column];
    }

    /**
     * A column of the batch in hand as verdicts, one per row: HyperCast's `Success` around
     * the value, or its `Fault`.
     *
     * @param int $column the plan column
     * @return list<Success|Fault> the verdicts, by row
     * @throws \OutOfRangeException when the plan has no such column
     */
    public function verdicts(int $column): array
    {
        $this->decode($column);
        $faults = $this->faults[$column];
        $verdicts = [];
        foreach ($this->values[$column] as $row => $value) {
            $verdicts[] = $faults[$row] ?? new Success($value);
        }
        return $verdicts;
    }

    /**
     * The verdict of one cell of the batch in hand: HyperCast's `Success` around the
     * value — the PHP carrier of the column's door ({@see Door}) — or its `Fault`, whose
     * span indexes the cell's own text ({@see raw()}).
     *
     * @param int $column the plan column
     * @param int $row the row within the batch
     * @return Success|Fault the verdict
     * @throws \OutOfRangeException when the plan has no such column or the batch no such row
     */
    public function cell(int $column, int $row): Success|Fault
    {
        $this->decode($column);
        if ($row < 0 || $row >= $this->rows) {
            throw new \OutOfRangeException("The batch has {$this->rows} rows; there is no row {$row}");
        }
        return $this->faults[$column][$row] ?? new Success($this->values[$column][$row]);
    }

    /**
     * The text a cell of the batch in hand was cast from, whatever its door and whatever
     * its verdict — what a fault's span indexes, and what to show for a value that did not
     * cast. Quotes are resolved, as the core resolved them before casting.
     *
     * @param int $column the plan column
     * @param int $row the row within the batch
     * @return string the cell's bytes
     * @throws \OutOfRangeException when the plan has no such column or the batch no such row
     */
    public function raw(int $column, int $row): string
    {
        if ($column < 0 || $column >= $this->columns) {
            throw new \OutOfRangeException("The plan has {$this->columns} columns; there is no column {$column}");
        }
        if ($row < 0 || $row >= $this->rows) {
            throw new \OutOfRangeException("The batch has {$this->rows} rows; there is no row {$row}");
        }
        $cell = $this->cells[$row * $this->perRow + $this->plan[$column]->ordinal];
        $length = $cell->len & Native::SPAN_LENGTH;
        if ($length === 0) {
            return '';
        }
        $raw = FFI::string($this->bufferPtr + ($this->windowAt + $cell->offset), $length);
        if (($cell->len & Native::SPAN_FLAG) === 0) {
            return $raw;
        }
        // A cell with an escaped quote in it: unescaped by the core, as the core cast it.
        if ($this->scratchCap < $length) {
            $this->scratchCap = max($length, 256);
            $this->scratch = $this->ffi->new("uint8_t[{$this->scratchCap}]");
        }
        $written = $this->ffi->hypertabular_delimited_unescape($raw, $length, $this->scratch, $this->scratchCap);
        return FFI::string($this->scratch, $written);
    }

    /**
     * Ends the read: the batch in hand is let go, and a file the reader opened itself
     * ({@see open()}) is closed. A stream the caller passed in stays open. Safe to call
     * more than once.
     *
     * @return void
     */
    public function close(): void
    {
        if ($this->closed) {
            return;
        }
        $this->closed = true;
        $this->rows = 0;
        $this->values = [];
        $this->faults = [];
        $this->window = null;
        $this->arenaBytes = null;
        $this->text = null;
        if ($this->ownsStream && \is_resource($this->stream)) {
            fclose($this->stream);
        }
        $this->stream = null;
    }

    /**
     * Fills the buffer for the first time and reads the header, if the dialect declares one.
     *
     * @param Dialect $dialect the declared dialect
     * @return void
     */
    private function begin(Dialect $dialect): void
    {
        if (!$this->eof) {
            $this->refill();
        }
        if ($dialect->hasHeader) {
            $this->readHeader();
        }
    }

    /**
     * The next bytes of the source, at most `$room` of them; an empty string is its end.
     *
     * @param int $room how many bytes the buffer has room for
     * @return string the bytes
     */
    private function pull(int $room): string
    {
        if ($this->text !== null) {
            $chunk = substr($this->text, $this->textAt, $room);
            $this->textAt += \strlen($chunk);
            if ($this->textAt >= \strlen($this->text)) {
                // Known without asking again, so the core hears `last` with the final bytes.
                $this->eof = true;
            }
            return $chunk;
        }
        $chunk = fread($this->stream, $room);
        if ($chunk === false) {
            throw new \RuntimeException('hypertabular: reading the stream failed');
        }
        if ($chunk === '' && !feof($this->stream)) {
            throw new \RuntimeException(
                'hypertabular: the stream returned no data before its end (a non-blocking stream, or a '
                . 'read that timed out); the reader needs one that blocks'
            );
        }
        return $chunk;
    }

    /**
     * Puts the unfinished record at the front of the buffer and reads more behind it,
     * doubling the buffer when one record fills it.
     *
     * @return void
     */
    private function refill(): void
    {
        $pending = $this->end - $this->start;
        if ($this->start > 0) {
            if ($pending > 0) {
                // Through a PHP string: the two ranges can overlap, and FFI has no memmove.
                FFI::memcpy($this->buffer, FFI::string($this->bufferPtr + $this->start, $pending), $pending);
            }
            $this->start = 0;
            $this->end = $pending;
        }
        if ($this->end === $this->capacity) {
            // One record fills the buffer: it needs a bigger one.
            if ($this->capacity >= self::MAX_ROW_BYTES) {
                $state = $this->state;
                throw $this->failure = new TabularException(
                    TabularFailure::RowTooLong,
                    $state->records,
                    $state->line,
                    $state->offset
                );
            }
            $capacity = min($this->capacity * 2, self::MAX_ROW_BYTES);
            $larger = $this->ffi->new("uint8_t[{$capacity}]");
            FFI::memcpy($larger, $this->buffer, $this->end);
            $this->buffer = $larger;
            $this->bufferPtr = $this->ffi->cast('uint8_t *', FFI::addr($larger));
            $this->capacity = $capacity;
        }
        $chunk = $this->pull($this->capacity - $this->end);
        if ($chunk === '') {
            $this->eof = true;
            return;
        }
        FFI::memcpy($this->bufferPtr + $this->end, $chunk, \strlen($chunk));
        $this->end += \strlen($chunk);
    }

    /**
     * A larger arena: what one row needs, or twice what there was, whichever is more.
     *
     * @param int $needed the arena bytes one row needs
     * @return void
     */
    private function growArena(int $needed): void
    {
        $this->arenaCap = max($needed, $this->arenaCap * 2);
        $this->arena = $this->ffi->new("uint8_t[{$this->arenaCap}]");
    }

    /**
     * The structural failure the core just reported, kept so that every later read throws
     * the same one.
     *
     * @return TabularException the failure
     */
    private function structural(): TabularException
    {
        $failure = $this->filled->failure;
        return $this->failure = new TabularException(
            $failure->code === 2 ? TabularFailure::ColumnCount : TabularFailure::UnclosedQuote,
            $failure->record,
            $failure->line,
            $failure->byte,
            $failure->expected,
            $failure->found
        );
    }

    /**
     * Reads the next record as the header.
     *
     * @return void
     */
    private function readHeader(): void
    {
        $ffi = $this->ffi;
        $filled = $this->filled;
        $namesCap = 64;
        $names = $ffi->new("ht_span[{$namesCap}]");
        while (true) {
            $length = $this->end - $this->start;
            $last = $this->eof;
            $window = $this->bufferPtr + $this->start;
            $code = $ffi->hypertabular_delimited_header(
                $this->statePtr,
                $window,
                $length,
                $last ? 1 : 0,
                $names,
                $namesCap,
                $this->arena,
                $this->arenaCap,
                $this->filledPtr
            );
            switch ($code) {
                case Native::OK:
                    $consumed = $filled->consumed;
                    $this->start += $consumed;
                    if ($filled->rows > 0) {
                        $input = FFI::string($window, $consumed);
                        $arena = FFI::string($this->arena, $filled->arena_used);
                        $header = [];
                        for ($index = 0; $index < $filled->rows; $index++) {
                            $name = $names[$index];
                            $header[] = substr(
                                ($name->len & Native::SPAN_FLAG) !== 0 ? $arena : $input,
                                $name->offset,
                                $name->len & Native::SPAN_LENGTH
                            );
                        }
                        $this->header = $header;
                        return;
                    }
                    if ($last && ($consumed === $length || $consumed === 0)) {
                        // An empty input has no header and no rows; the width is unknown.
                        $this->header = [];
                        return;
                    }
                    if ($consumed === 0) {
                        $this->refill();
                    }
                    break;
                case Native::ERR_CELLS:
                    $namesCap = $filled->needed;
                    $names = $ffi->new("ht_span[{$namesCap}]");
                    break;
                case Native::ERR_ARENA:
                    $this->growArena($filled->needed);
                    break;
                case Native::ERR_STRUCTURE:
                    throw $this->structural();
                default:
                    throw new \RuntimeException(self::CONTRACT_VIOLATION);
            }
        }
    }

    /**
     * Lifts one column of the batch in hand out of the core's buffers — its verdict array
     * and its value array, each read as one string and unpacked — unless it already has
     * been.
     *
     * @param int $column the plan column
     * @return void
     * @throws \OutOfRangeException when the plan has no such column
     */
    private function decode(int $column): void
    {
        if (isset($this->values[$column])) {
            return;
        }
        if ($column < 0 || $column >= $this->columns) {
            throw new \OutOfRangeException("The plan has {$this->columns} columns; there is no column {$column}");
        }
        $rows = $this->rows;
        if ($rows === 0) {
            $this->values[$column] = [];
            $this->faults[$column] = [];
            return;
        }

        // A verdict is three little-endian u32s — offset, length, reason — and reason 0 is
        // a cell that cast. unpack() numbers from 1.
        $faults = [];
        $verdicts = unpack('V*', FFI::string($this->verdictBuffers[$column], $rows * 12));
        for ($row = 0, $at = 3; $row < $rows; $row++, $at += 3) {
            if ($verdicts[$at] !== 0) {
                $faults[$row] = new Fault(CastFailure::from($verdicts[$at]), $verdicts[$at - 2], $verdicts[$at - 1]);
            }
        }

        $door = $this->plan[$column]->door;
        $bytes = FFI::string($this->valueBuffers[$column], $rows * $door->valueSize());
        $values = match ($door) {
            Door::Bool => array_map(static fn (int $byte): bool => $byte !== 0, array_values(unpack('C*', $bytes))),
            Door::I8 => array_values(unpack('c*', $bytes)),
            Door::I16 => array_values(unpack('s*', $bytes)),
            Door::I32 => array_values(unpack('l*', $bytes)),
            // u64 rides PHP's signed int as its two's-complement bit pattern, as HyperCast's
            // own u64 door presents it; a time of day is far inside the signed range.
            Door::I64, Door::U64, Door::Time => array_values(unpack('q*', $bytes)),
            Door::U8 => array_values(unpack('C*', $bytes)),
            Door::U16 => array_values(unpack('v*', $bytes)),
            Door::U32 => array_values(unpack('V*', $bytes)),
            Door::F32 => array_values(unpack('g*', $bytes)),
            Door::F64 => array_values(unpack('e*', $bytes)),
            default => $this->structured($door, $bytes, $rows, $faults),
        };
        foreach ($faults as $row => $_) {
            $values[$row] = null;
        }
        $this->values[$column] = $values;
        $this->faults[$column] = $faults;
    }

    /**
     * The values of a column whose door writes more than one number per cell, built into
     * the carriers HyperCast's `Cast` returns for the same doors. A cell that did not cast
     * is null here already, and is never built.
     *
     * @param Door $door the column's door
     * @param string $bytes the column's value array
     * @param int $rows rows in the batch
     * @param array<int, Fault> $faults the column's faults, by row
     * @return list<mixed> the values, by row
     */
    private function structured(Door $door, string $bytes, int $rows, array $faults): array
    {
        $values = [];
        for ($row = 0; $row < $rows; $row++) {
            if (isset($faults[$row])) {
                $values[] = null;
                continue;
            }
            switch ($door) {
                case Door::Decimal:
                    $raw = unpack('qlo/Vhi/Cscale/Cnegative', $bytes, $row * 16);
                    $values[] = Decimal::fromLimbs($raw['lo'], $raw['hi'], $raw['scale'], $raw['negative'] !== 0);
                    break;
                case Door::Uuid:
                    $hex = bin2hex(substr($bytes, $row * 16, 16));
                    $values[] = substr($hex, 0, 8) . '-' . substr($hex, 8, 4) . '-' . substr($hex, 12, 4)
                        . '-' . substr($hex, 16, 4) . '-' . substr($hex, 20);
                    break;
                case Door::Timestamp:
                case Door::Unix:
                case Door::ExcelSerial:
                    $raw = unpack('qseconds/lnanos', $bytes, $row * 16);
                    $values[] = self::instant($raw['seconds'], intdiv($raw['nanos'], 1000));
                    break;
                case Door::Date:
                case Door::DateOrdered:
                    $raw = unpack('vyear/Cmonth/Cday', $bytes, $row * 4);
                    $values[] = self::instant(self::epochSeconds($raw['year'], $raw['month'], $raw['day']), 0);
                    break;
                case Door::DateTime:
                    $raw = unpack('vyear/Cmonth/Cday/x4/Pnanos', $bytes, $row * 16);
                    $values[] = self::instant(
                        self::epochSeconds($raw['year'], $raw['month'], $raw['day'])
                            + intdiv($raw['nanos'], 1_000_000_000),
                        intdiv($raw['nanos'] % 1_000_000_000, 1000)
                    );
                    break;
                case Door::Duration:
                    $raw = unpack('qseconds/lnanos', $bytes, $row * 16);
                    $values[] = new Duration($raw['seconds'], $raw['nanos']);
                    break;
                case Door::Text:
                    // A span into the input the batch was read from — or, flagged, into the
                    // arena, where the core unescaped the cell.
                    $raw = unpack('Voffset/Vlen', $bytes, $row * 8);
                    $values[] = ($raw['len'] & Native::SPAN_FLAG) !== 0
                        ? substr($this->arenaText(), $raw['offset'], $raw['len'] & Native::SPAN_LENGTH)
                        : substr($this->windowText(), $raw['offset'], $raw['len']);
                    break;
                default:
                    throw new \LogicException("No carrier for door {$door->name}");
            }
        }
        return $values;
    }

    /**
     * The input the batch in hand was read from, lifted into PHP once per batch.
     *
     * @return string the bytes the core consumed for this batch
     */
    private function windowText(): string
    {
        return $this->window ??= FFI::string($this->bufferPtr + $this->windowAt, $this->windowBytes);
    }

    /**
     * What the core wrote to the arena for the batch in hand, lifted into PHP once per batch.
     *
     * @return string the unescaped cells, end to end
     */
    private function arenaText(): string
    {
        return $this->arenaBytes ??= FFI::string($this->arena, $this->arenaUsed);
    }

    /**
     * A UTC instant, built as HyperCast's `Cast` builds one: createFromTimestamp and
     * setMicrosecond on PHP 8.4+, the date-string fallback below it.
     *
     * @param int $seconds seconds since the epoch
     * @param int $micros microseconds within the second
     * @return DateTimeImmutable the instant
     */
    private static function instant(int $seconds, int $micros): DateTimeImmutable
    {
        if (self::$fastInstants) {
            $instant = DateTimeImmutable::createFromTimestamp($seconds);
            return $micros === 0 ? $instant : $instant->setMicrosecond($micros);
        }
        $instant = new DateTimeImmutable("@{$seconds}");
        return $micros === 0 ? $instant : $instant->modify("+{$micros} microseconds");
    }

    /**
     * Epoch seconds at midnight of a civil date — Hinnant's days_from_civil, the same math
     * the core and HyperCast's `Cast` use.
     *
     * @param int $year the civil year
     * @param int $month the civil month
     * @param int $day the civil day
     * @return int seconds since the epoch at that date's midnight
     */
    private static function epochSeconds(int $year, int $month, int $day): int
    {
        $shifted = $month <= 2 ? $year - 1 : $year;
        $era = intdiv($shifted >= 0 ? $shifted : $shifted - 399, 400);
        $yearOfEra = $shifted - $era * 400;
        $dayOfYear = intdiv(153 * ($month + ($month > 2 ? -3 : 9)) + 2, 5) + $day - 1;
        $dayOfEra = $yearOfEra * 365 + intdiv($yearOfEra, 4) - intdiv($yearOfEra, 100) + $dayOfYear;
        return ($era * 146_097 + $dayOfEra - 719_468) * 86_400;
    }
}
