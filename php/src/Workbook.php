<?php

declare(strict_types=1);

namespace HyperTabular;

use FFI;
use FFI\CData;
use HyperCast\ExcelEpoch;

/**
 * An XLSX or ODS workbook held in memory, its sheets listed and its shared strings and
 * styles loaded: what a {@see Sheet} reads from.
 *
 * ```php
 * $book = Workbook::open('orders.xlsx');
 * $sheet = $book->sheet('Orders', new SheetOptions(), $plan);
 * while (($batch = $sheet->read()) !== null) { ... }
 *
 * // Or the header first, and the plan by name.
 * $sheet = $book->sheet('Orders', new SheetOptions());
 * $header = $sheet->headerIndex();
 * $sheet->bind([Column::i32($header->ordinal('id')), Column::text($header->ordinal('name'))]);
 * foreach ($sheet->rows() as $row) { ... }
 * ```
 *
 * A workbook that cannot be read — not a zip, encrypted, a part missing or broken — is a
 * {@see TabularException} from the factory. Its sheets may be read at once, each with
 * buffers of its own.
 */
final class Workbook
{
    /** The smallest window the core inflates a part through. */
    private const WINDOW_MIN = 64 * 1024;

    private CData $container;
    private int $length;
    /** The state as opening left it: the template every sheet's own is copied from. */
    private CData $state;
    private int $stateBytes;
    private WorkbookFormat $format;
    private ExcelEpoch $dateSystem;
    /** @var list<SheetInfo> */
    private array $sheets = [];
    private string $strings;
    private CData $stringsBuffer;
    private int $stringsLen;
    private CData $table;
    private int $tableLen;
    private CData $kinds;
    private int $kindsLen;

    /**
     * Opens a workbook held in a string, copied once into native memory.
     *
     * @param string $bytes the workbook's bytes
     * @return self the workbook, its sheets listed and its tables loaded
     * @throws TabularException when the bytes are not a workbook this reader can read
     */
    public static function fromString(string $bytes): self
    {
        return new self($bytes);
    }

    /**
     * Reads a stream to its end and opens what it held. The workbook keeps its own copy; the
     * stream stays the caller's, as it does for {@see DelimitedReader::fromStream()} — read
     * from where it stands to its end, never sought, and not closed. Which kind of workbook
     * it is, is told from the bytes.
     *
     * @param resource $stream a readable stream
     * @return self the workbook
     * @throws \InvalidArgumentException when `$stream` is not a stream
     * @throws \RuntimeException when the stream cannot be read to its end
     * @throws TabularException when the bytes are not a workbook this reader can read
     */
    public static function fromStream($stream): self
    {
        if (!\is_resource($stream) || get_resource_type($stream) !== 'stream') {
            throw new \InvalidArgumentException('The stream must be an open stream resource');
        }
        $bytes = stream_get_contents($stream);
        if ($bytes === false) {
            throw new \RuntimeException('hypertabular: reading the stream failed');
        }
        if (!feof($stream)) {
            throw new \RuntimeException(
                'hypertabular: the stream stopped before its end (a non-blocking stream, or a read that '
                . 'timed out); the workbook needs one that blocks'
            );
        }
        return new self($bytes);
    }

    /**
     * Reads the file at `$path` and opens it.
     *
     * @param string $path the workbook
     * @return self the workbook
     * @throws \RuntimeException when the file cannot be read
     * @throws TabularException when the file is not a workbook this reader can read
     */
    public static function open(string $path): self
    {
        $bytes = @file_get_contents($path);
        if ($bytes === false) {
            $reason = error_get_last()['message'] ?? 'unknown error';
            throw new \RuntimeException("hypertabular: cannot open {$path}: {$reason}");
        }
        return new self($bytes);
    }

    /**
     * Opens the container: its state, its sheets, its tables.
     *
     * @param string $bytes the workbook's bytes
     * @throws TabularException when the bytes are not a workbook this reader can read
     */
    private function __construct(string $bytes)
    {
        $ffi = Native::ffi();
        $this->length = \strlen($bytes);
        $this->container = $ffi->new('uint8_t[' . max(1, $this->length) . ']');
        if ($this->length > 0) {
            FFI::memcpy($this->container, $bytes, $this->length);
        }
        $this->stateBytes = $ffi->hypertabular_workbook_state_size();
        $this->state = $ffi->new('uint64_t[' . intdiv($this->stateBytes + 7, 8) . ']');
        $scratch = Scratch::$stingy ? new Scratch(1, 1, 1, 0) : new Scratch(self::WINDOW_MIN, 1024, 64, 0);

        // Opening starts over when it is refused, and reports through its own shape.
        $opened = $ffi->new('ht_opened');
        while (true) {
            $code = $ffi->hypertabular_workbook_open(
                $this->state,
                $this->container,
                $this->length,
                $scratch->buffers(),
                FFI::addr($opened)
            );
            if ($code === Native::OK) {
                break;
            }
            match ($code) {
                Native::ERR_WINDOW => $scratch->growWindow($opened->needed),
                Native::ERR_ARENA => $scratch->growArena($opened->needed),
                Native::ERR_STRUCTURE => throw TabularException::from($opened->failure),
                default => throw new \RuntimeException(
                    'hypertabular: libhypertabular reported a contract violation — a binding bug, please report it'
                ),
            };
        }
        $this->format = WorkbookFormat::from($opened->format);
        $this->dateSystem = ExcelEpoch::from($opened->epoch);

        // The sheets: three spans each — the name and the part, in the arena, then one whose
        // offset's low bit says hidden and whose length is the sheet's index.
        $this->run($scratch, 'hypertabular_workbook_sheets');
        $rows = $scratch->filled->rows;
        $arena = FFI::string($scratch->arena, $scratch->arenaCap);
        for ($index = 0; $index < $rows; $index++) {
            $name = $scratch->cells[$index * 3];
            $part = $scratch->cells[$index * 3 + 1];
            $last = $scratch->cells[$index * 3 + 2];
            $this->sheets[] = new SheetInfo(
                substr($arena, $name->offset, $name->len),
                ($last->offset & 1) !== 0,
                substr($arena, $part->offset, $part->len),
                $last->len
            );
        }

        // The shared strings take no more room than their part inflates to; asking for it
        // once saves growing into it.
        $bound = min($opened->strings_bytes, 1 << 28);
        if (!Scratch::$stingy && $scratch->arenaCap < $bound) {
            $scratch->growArena($bound);
        }
        $this->run($scratch, 'hypertabular_workbook_strings');
        $this->stringsLen = $scratch->filled->arena_used;
        $this->strings = FFI::string($scratch->arena, $this->stringsLen);
        $this->stringsBuffer = self::copy($scratch->arena, $this->stringsLen, 'uint8_t', 1);
        $this->tableLen = $scratch->filled->rows;
        $this->table = self::copy($scratch->cells, $this->tableLen, 'ht_span', 8);

        $this->run($scratch, 'hypertabular_workbook_styles');
        $this->kindsLen = $scratch->filled->rows;
        $this->kinds = self::copy($scratch->arena, $this->kindsLen, 'uint8_t', 1);
    }

    /**
     * Makes one of the calls that list and load, settled.
     *
     * @param Scratch $scratch the buffers it works in
     * @param string $function the export
     * @return void
     */
    private function run(Scratch $scratch, string $function): void
    {
        $ffi = Native::ffi();
        $code = $scratch->drive(fn (CData $buffers): int => $ffi->$function(
            $this->state,
            $this->container,
            $this->length,
            $buffers,
            FFI::addr($scratch->filled)
        ));
        Scratch::settle($code, $scratch->filled);
    }

    /**
     * The first `$count` elements of an array, copied into one of their own.
     *
     * @param CData $from the array
     * @param int $count its elements to copy
     * @param string $type its element type
     * @param int $size bytes an element takes
     * @return CData the copy
     */
    private static function copy(CData $from, int $count, string $type, int $size): CData
    {
        $copy = Native::ffi()->new("{$type}[" . max(1, $count) . ']');
        if ($count > 0) {
            FFI::memcpy($copy, $from, $count * $size);
        }
        return $copy;
    }

    /**
     * Which kind of workbook this is.
     *
     * @return WorkbookFormat XLSX or ODS
     */
    public function format(): WorkbookFormat
    {
        return $this->format;
    }

    /**
     * The date system the workbook's serials count in: what a date-formatted number is read
     * by.
     *
     * @return ExcelEpoch HyperCast's own
     */
    public function dateSystem(): ExcelEpoch
    {
        return $this->dateSystem;
    }

    /**
     * The workbook's sheets, in its own order. Sheets that hold no cells (chart sheets,
     * macro sheets) are not among them.
     *
     * @return list<SheetInfo> the sheets
     */
    public function sheets(): array
    {
        return $this->sheets;
    }

    /**
     * Starts a read of one sheet — `$which` is its index in {@see sheets()} or its name —
     * through `$plan`. With a header declared the header row is read here. Without a plan
     * the sheet reads its header and waits for one: {@see Sheet::bind()}.
     *
     * @param int|string $which the sheet's index, or its name
     * @param SheetOptions $options how the sheet is read
     * @param list<Column>|null $plan the output columns, in output order — or null to read
     *     the header first and bind a plan after
     * @return Sheet the sheet
     * @throws \OutOfRangeException when the workbook has no sheet at that index
     * @throws \OutOfBoundsException when the workbook has no sheet by that name
     * @throws \InvalidArgumentException when the plan cannot be honoured
     * @throws TabularException when the sheet, or its header row, is structurally broken
     */
    public function sheet(int|string $which, SheetOptions $options, ?array $plan = null): Sheet
    {
        if (\is_int($which)) {
            $info = $this->sheets[$which] ?? throw new \OutOfRangeException(
                'The workbook has ' . \count($this->sheets) . " sheets; there is no sheet {$which}"
            );
            return new Sheet($this, $info, $options, $plan);
        }
        foreach ($this->sheets as $info) {
            if ($info->name === $which) {
                return new Sheet($this, $info, $options, $plan);
            }
        }
        throw new \OutOfBoundsException("The workbook has no sheet named \"{$which}\"");
    }

    /**
     * The container, as a sheet's calls are handed it.
     *
     * @return array{CData, int} the bytes and their length
     * @internal
     */
    public function container(): array
    {
        return [$this->container, $this->length];
    }

    /**
     * A copy of the opened state, which the core allows: a sheet's read of its own.
     *
     * @return CData the copy
     * @internal
     */
    public function copyOfState(): CData
    {
        $copy = Native::ffi()->new('uint64_t[' . intdiv($this->stateBytes + 7, 8) . ']');
        FFI::memcpy($copy, $this->state, $this->stateBytes);
        return $copy;
    }

    /**
     * The shared strings' bytes, which every batch of every sheet indexes.
     *
     * @return string the bytes
     * @internal
     */
    public function strings(): string
    {
        return $this->strings;
    }

    /**
     * Writes the workbook's tables into a buffers block, for a call that reads a sheet.
     *
     * @param CData $buffers an `ht_buffers`
     * @return void
     * @internal
     */
    public function tables(CData $buffers): void
    {
        $ffi = Native::ffi();
        $buffers->strings = $ffi->cast('uint8_t *', FFI::addr($this->stringsBuffer));
        $buffers->strings_len = $this->stringsLen;
        $buffers->table = $ffi->cast('ht_span *', FFI::addr($this->table));
        $buffers->table_len = $this->tableLen;
        $buffers->kinds = $ffi->cast('uint8_t *', FFI::addr($this->kinds));
        $buffers->kinds_len = $this->kindsLen;
    }
}
