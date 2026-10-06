<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;
use FFI\CData;

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
 * batch, not once per cell; {@see read()} returns the rows as a {@see Batch}.
 *
 * A value that does not cast is that cell's verdict, and the read goes on. Input that is
 * not rows of cells at all — a record of the wrong width, a quote never closed — is a
 * {@see TabularException}, thrown after every intact row before it has been delivered.
 *
 * HyperCast is the judge: a verdict is HyperCast's own `Success` or `Fault`, a fault's
 * reason its `CastFailure`, and a value the same PHP carrier HyperCast's `Cast` returns
 * for that door ({@see Door} lists them).
 *
 * A batch owns what it shows, and stays good after the next {@see read()}. Not safe to
 * share between threads or fibers that read concurrently.
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

    private FFI $ffi;

    private Dialect $dialect;
    private Columns $columns;
    /** Cell-table entries one row takes: the widest ordinal the plan reads, plus two — or more, if the core asked. */
    private int $perRow;

    // Everything the core is handed, allocated once. A CData that owns memory is kept
    // beside every pointer into it: the pointer does not keep the memory alive.
    private CData $state;
    private CData $statePtr;
    private CData $filled;
    private CData $filledPtr;
    private CData $cells;
    private int $cellsCap;
    private CData $arena;
    private int $arenaCap;
    /** Whether the last batch ended early because the arena filled: the next starts with it doubled. */
    private bool $cramped = false;

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
        $this->columns = new Columns($plan, $batchRows);
        $ffi = $this->ffi = Native::ffi();
        $this->dialect = $dialect;
        $this->perRow = $this->columns->width + 1;
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
     * The plan the reader was built with: column `i` of every batch is `plan()[i]`.
     *
     * @return list<Column> the output columns, in output order
     */
    public function plan(): array
    {
        return $this->columns->plan;
    }

    /**
     * The declared dialect.
     *
     * @return Dialect the dialect the text is read in
     */
    public function dialect(): Dialect
    {
        return $this->dialect;
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
     * Reads the next batch: one native call fills every column. A {@see Batch} of up to the
     * reader's batch size of rows, or null once the input is exhausted.
     *
     * @return Batch|null the rows, or null at the end
     * @throws TabularException when the input is structurally broken — thrown after every
     *     intact row before the break has been delivered, and again on every later call
     * @throws \LogicException when the reader has been closed
     * @throws \RuntimeException when the stream cannot be read
     */
    public function read(): ?Batch
    {
        if ($this->closed) {
            throw new \LogicException('The reader is closed');
        }
        if ($this->failure !== null) {
            throw $this->failure;
        }
        if ($this->cramped) {
            $this->growArena($this->arenaCap * 2);
            $this->cramped = false;
        }
        $ffi = $this->ffi;
        $filled = $this->filled;
        $columns = $this->columns;
        while (true) {
            $length = $this->end - $this->start;
            $last = $this->eof;
            $code = $ffi->hypertabular_delimited_fill(
                $this->statePtr,
                $this->bufferPtr + $this->start,
                $length,
                $last ? 1 : 0,
                $columns->specs,
                $columns->buffers,
                $columns->count,
                $columns->batchRows,
                $this->cells,
                $this->cellsCap,
                $this->arena,
                $this->arenaCap,
                $this->filledPtr
            );
            switch ($code) {
                case Native::OK:
                    $consumed = $filled->consumed;
                    $rows = $filled->rows;
                    if ($rows > 0) {
                        $this->cramped = $rows < $columns->batchRows && $filled->arena_used * 2 >= $this->arenaCap;
                        $batch = new Batch(
                            $columns->plan,
                            $rows,
                            $columns->values($rows),
                            $columns->verdicts($rows),
                            FFI::string($this->cells, $rows * $this->perRow * 8),
                            $this->perRow,
                            FFI::string($this->bufferPtr + $this->start, $consumed),
                            FFI::string($this->arena, $filled->arena_used),
                            false
                        );
                        $this->start += $consumed;
                        return $batch;
                    }
                    $this->start += $consumed;
                    if ($last && ($consumed === $length || $consumed === 0)) {
                        return null;
                    }
                    if ($consumed === 0) {
                        $this->refill();
                    }
                    break;
                case Native::ERR_CELLS:
                    $this->perRow = max($this->perRow, $filled->needed);
                    $this->cellsCap = $this->perRow * $columns->batchRows;
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
     * Ends the read: a file the reader opened itself ({@see open()}) is closed. A stream the
     * caller passed in stays open, and batches already read stay good. Safe to call more
     * than once.
     *
     * @return void
     */
    public function close(): void
    {
        if ($this->closed) {
            return;
        }
        $this->closed = true;
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
        return $this->failure = TabularException::from($this->filled->failure);
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
}
