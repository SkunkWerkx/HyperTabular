<?php

/**
 * Stubs for the hypertabular extension, for IDEs and static analysis. Generated: don't edit.
 * Regenerate from php/ with `php tools/generate-stubs.php`, which runs
 * `cargo php stubs --stdout` (cargo-php 0.2.1) on the debug build of the extension and
 * completes its output (the tool says how).
 */

namespace HyperTabular {
    /**
     * The rows one `DelimitedReader::read()` or `Sheet::read()` delivered, as typed columns: the
     * same class for delimited text and for a sheet of a workbook.
     *
     * ```php
     * while (($batch = $reader->read()) !== null) {
     *     $ids = $batch->values(0);                    // a column at a time
     *     for ($row = 0; $row < $batch->rows(); $row++) {
     *         $verdict = $batch->get(2, $row);         // or a cell at a time, as HyperCast's union
     *         if ($verdict instanceof Fault) {
     *             echo "line {$batch->line($row)}: {$verdict->reason->name} in {$batch->raw(2, $row)}\n";
     *         }
     *     }
     * }
     * ```
     *
     * Or read across, a `Row` at a time: a batch iterates its rows in order. A batch owns what it
     * shows — the arrays the core wrote are copied out of the reader's buffers as it is made — so
     * it stays good after the reader has moved on. A value is the PHP carrier HyperCast's `Cast`
     * returns for the column's door (`Door` lists them).
     */
    final class Batch implements \IteratorAggregate {
        /**
         * Made by a reader or a sheet as it reads; not constructible.
         */
        private function __construct() {}

        /**
         * The plan the batch was read through: column `i` of the batch is `columns()[i]`.
         *
         * @return array
         */
        public function columns(): array {}

        /**
         * The cells of a column that did not cast: HyperCast's `Fault` for each, keyed by row. Empty
         * when every cell cast.
         *
         * @param int $column
         * @return array
         */
        public function faults(int $column): array {}

        /**
         * The cell at (`$column`, `$row`) as HyperCast judged it: its `Success` around the value —
         * the PHP carrier of the column's door (`Door`) — or its `Fault`, whose span indexes the
         * cell's own text (`raw()`).
         *
         * @param int $column
         * @param int $row
         * @return \HyperCast\Success|\HyperCast\Fault
         */
        public function get(int $column, int $row): \HyperCast\Success|\HyperCast\Fault {}

        /**
         * The batch's rows, in order, each a `Row` — keyed by its index — that answers as the batch
         * does for that row.
         *
         * @return \Iterator
         */
        public function getIterator(): \Iterator {}

        /**
         * Where a row came from: for delimited text the 1-based line its record starts on, for a
         * sheet its 1-based row number.
         *
         * @param int $row
         * @return int
         */
        public function line(int $row): int {}

        /**
         * The text a cell was cast from, whatever its door and whatever its verdict — what a fault's
         * span indexes, and what to show for a value that did not cast. Quotes are resolved, as the
         * core resolved them before casting. For a sheet this is a text cell's own text, and what a
         * typed cell was said as when it failed its door (or went through the text door); a typed
         * cell that cast has none.
         *
         * @param int $column
         * @param int $row
         * @return string
         */
        public function raw(int $column, int $row): string {}

        /**
         * How many rows the batch holds — never zero.
         *
         * @return int
         */
        public function rows(): int {}

        /**
         * A column, one value per row, each the PHP carrier of the column's door (`Door`) — and null
         * where the cell did not cast, whatever the reason; `faults()` says why. Built in one piece
         * the first time it is asked for.
         *
         * @param int $column
         * @return array
         */
        public function values(int $column): array {}

        /**
         * A column as verdicts, one per row: HyperCast's `Success` around the value, or its `Fault`.
         *
         * @param int $column
         * @return array
         */
        public function verdicts(int $column): array {}
    }

    /**
     * Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
     * typed columns, every cell a HyperCast `Success|Fault` verdict.
     *
     * The core owns no memory and reads no files. It is handed a chunk of input and the buffers
     * to fill, casts each plan column in one loop, and says how many rows it wrote and how many
     * bytes it is finished with. This class allocates the input buffer, one value array and one
     * verdict array per column, the table that locates each cell and the arena for the rare
     * escaped cell — once, reused for every batch — and puts what the core did not consume back
     * in front of it. `read()` returns the rows as a `Batch`.
     *
     * A value that does not cast is that cell's verdict, and the read goes on. Input that is not
     * rows of cells at all — a record of the wrong width, a quote never closed — is a
     * `TabularException`, thrown after every intact row before it has been delivered, and thrown
     * again by every read after.
     *
     * HyperCast is the judge: a verdict is HyperCast's own `Success` or `Fault`, a fault's reason
     * its `CastFailure`, and a value the same PHP carrier HyperCast's `Cast` returns for that door
     * (`Door` lists them).
     *
     * The plan can wait for the header. Opened without one, a reader reads the header and stops:
     * `headerIndex()` looks each name up, and `bind()` declares the plan the names resolved to,
     * before the first read.
     *
     * ```php
     * $reader = DelimitedReader::open('data.csv', Dialect::csv());
     * $header = $reader->headerIndex();
     * $reader->bind([Column::i64($header->ordinal('id')), Column::text($header->ordinal('name'))]);
     * foreach ($reader->rows() as $row) {
     *     $id = $row->get(0);                          // Success or Fault
     * }
     * ```
     */
    final class DelimitedReader {
        /**
         * The rows a batch holds unless the caller says otherwise.
         */
        public const DEFAULT_BATCH_ROWS = 4096;

        /**
         * The input buffer a reader starts with, in bytes; it doubles for a record that does not fit.
         */
        public const DEFAULT_BUFFER_BYTES = 262144;

        /**
         * The longest a single record may be: 1 GiB. A record that outgrows it is a
         * `TabularFailure::RowTooLong`.
         */
        public const MAX_ROW_BYTES = 1073741824;

        /**
         * Made by `fromString()`, `fromStream()` or `open()`; not constructible.
         */
        private function __construct() {}

        /**
         * Closes the file `open()` opened.
         */
        public function __destruct() {}

        /**
         * Declares the plan, once, before the first read: which source column, through which door.
         *
         * @throws \LogicException when the reader already has a plan.
         * @throws \InvalidArgumentException for a plan entry that is not a `Column`, or a plan
         *                                   column past the header's width.
         *
         * @param array $plan
         * @return void
         */
        public function bind(array $plan): void {}

        /**
         * Lets go of the input, closing the file `open()` opened. A closed reader reads no more.
         *
         * @return void
         */
        public function close(): void {}

        /**
         * How many cells every record has — the width of the first record, the header's when there
         * is one — or null before one has been read.
         *
         * @return int|null
         */
        public function columnCount(): ?int {}

        /**
         * The dialect the reader reads.
         *
         * @return \HyperTabular\Dialect
         */
        public function dialect(): \HyperTabular\Dialect {}

        /**
         * A reader over an open stream, read a buffer at a time. The stream stays the caller's: the
         * reader never closes it.
         *
         * @throws \InvalidArgumentException as `fromString()`, and for anything but an open stream.
         * @throws \RuntimeException when reading the stream fails.
         * @throws TabularException when the header itself is broken.
         *
         * @param mixed $stream
         * @param \HyperTabular\Dialect $dialect
         * @param array|null $plan
         * @param int $batchRows
         * @param int $bufferBytes
         * @return \HyperTabular\DelimitedReader
         */
        public static function fromStream(mixed $stream, \HyperTabular\Dialect $dialect, ?array $plan = null, int $batchRows = self::DEFAULT_BATCH_ROWS, int $bufferBytes = self::DEFAULT_BUFFER_BYTES): \HyperTabular\DelimitedReader {}

        /**
         * A reader over `$utf8`, held in memory and handed to the core a buffer at a time.
         *
         * @throws \InvalidArgumentException for a batch or buffer of no rows or bytes, a plan entry
         *                                   that is not a `Column`, a separator the core refuses or
         *                                   a plan column past the header's width.
         * @throws TabularException when the header itself is broken.
         *
         * @param string $utf8
         * @param \HyperTabular\Dialect $dialect
         * @param array|null $plan
         * @param int $batchRows
         * @param int $bufferBytes
         * @return \HyperTabular\DelimitedReader
         */
        public static function fromString(string $utf8, \HyperTabular\Dialect $dialect, ?array $plan = null, int $batchRows = self::DEFAULT_BATCH_ROWS, int $bufferBytes = self::DEFAULT_BUFFER_BYTES): \HyperTabular\DelimitedReader {}

        /**
         * The header's names, as the source wrote them; an empty list for an empty input; null when
         * the dialect says there is no header.
         *
         * @return array|null
         */
        public function header(): ?array {}

        /**
         * The header as a `Header`, to look columns up by name; null when there is none.
         *
         * @return \HyperTabular\Header|null
         */
        public function headerIndex(): ?\HyperTabular\Header {}

        /**
         * Whether the reader has a plan.
         *
         * @return bool
         */
        public function isBound(): bool {}

        /**
         * A reader over the file at `$path`, opened with `fopen($path, 'rb')` — any stream wrapper
         * PHP knows — and closed by `close()` or when the reader is destroyed.
         *
         * @throws \RuntimeException when the file cannot be opened or read.
         * @throws \InvalidArgumentException as `fromString()`.
         * @throws TabularException when the header itself is broken.
         *
         * @param string $path
         * @param \HyperTabular\Dialect $dialect
         * @param array|null $plan
         * @param int $batchRows
         * @param int $bufferBytes
         * @return \HyperTabular\DelimitedReader
         */
        public static function open(string $path, \HyperTabular\Dialect $dialect, ?array $plan = null, int $batchRows = self::DEFAULT_BATCH_ROWS, int $bufferBytes = self::DEFAULT_BUFFER_BYTES): \HyperTabular\DelimitedReader {}

        /**
         * The plan, as bound; an empty list before one is.
         *
         * @return array
         */
        public function plan(): array {}

        /**
         * The next batch of rows, or null at the end of the input.
         *
         * @throws \LogicException when the reader is closed or has no plan.
         * @throws TabularException at a structural failure, once every intact row before it has
         *                          been delivered — and again at every read after.
         * @throws \RuntimeException when reading the stream fails.
         *
         * @return \HyperTabular\Batch|null
         */
        public function read(): ?\HyperTabular\Batch {}

        /**
         * How many records the core has finished with, the header's included.
         *
         * @return int
         */
        public function records(): int {}

        /**
         * Every row the reader has left, a batch at a time, each a `Row` keyed by its place in the
         * read: an iterator that reads as it goes, forward only.
         *
         * @return \Iterator
         */
        public function rows(): \Iterator {}
    }

    /**
     * The door a column is cast through: HyperCast's, plus `Door::Text` for the bytes
     * themselves. Named as HyperCast's PHP binding names its doors, and numbered as the core
     * numbers them. Each case says what a cell of that door is in PHP — the same carrier
     * HyperCast's own `Cast` returns for it.
     */
    enum Door: int {
      case Bool = 1;
      case I8 = 2;
      case I16 = 3;
      case I32 = 4;
      case I64 = 5;
      case U8 = 6;
      case U16 = 7;
      case U32 = 8;
      case U64 = 9;
      case F32 = 10;
      case F64 = 11;
      case Uuid = 12;
      case Timestamp = 13;
      case Unix = 14;
      case Date = 15;
      case Time = 16;
      case Duration = 17;
      case Text = 18;
      case Decimal = 19;
      case DateOrdered = 20;
      case DateTime = 21;
      case ExcelSerial = 22;

        /**
         * Bytes one value of this door takes in a column buffer: the core's layout.
         */
        public function valueSize(): int {}
    }

    /**
     * The names a source's header row declared, and the ordinal each is at: what a plan is built
     * from when the columns are known by name.
     *
     * ```php
     * $header = $reader->headerIndex();
     * $plan = [Column::i64($header->ordinal('id')), Column::text($header->ordinal('name'))];
     * $reader->bind($plan);
     * ```
     *
     * A name is matched exactly, byte for byte; where a name repeats, the first column of that
     * name is the one found. A header is read-only: it counts, iterates and indexes as the list of
     * its names, and serializes to JSON as that list.
     */
    final class Header implements \ArrayAccess, \Countable, \IteratorAggregate, \JsonSerializable {
        /**
         * A header naming `$names`, in order: the first names column 0.
         *
         * @param array $names
         */
        public function __construct(array $names) {}

        /**
         * How many columns the header names.
         *
         * @return int
         */
        public function count(): int {}

        /**
         * The ordinal of the first column named `$name` exactly, or null when none is.
         *
         * @param string $name
         * @return int|null
         */
        public function find(string $name): ?int {}

        /**
         * The names, keyed by ordinal.
         *
         * @return \ArrayIterator
         */
        public function getIterator(): \ArrayIterator {}

        /**
         * The names, in column order.
         *
         * @return array
         */
        public function jsonSerialize(): array {}

        /**
         * The names, in column order.
         *
         * @return array
         */
        public function names(): array {}

        /**
         * Whether `$offset` is the ordinal of a column.
         *
         * @param mixed $offset
         * @return bool
         */
        public function offsetExists(mixed $offset): bool {}

        /**
         * The name of the column at ordinal `$offset`.
         *
         * @throws \OutOfRangeException when there is no such column.
         *
         * @param mixed $offset
         * @return mixed
         */
        public function offsetGet(mixed $offset): mixed {}

        /**
         * Refused: a header cannot be changed.
         *
         * @param mixed $offset
         * @param mixed $value
         * @return void
         */
        public function offsetSet(mixed $offset, mixed $value): void {}

        /**
         * Refused: a header cannot be changed.
         *
         * @param mixed $offset
         * @return void
         */
        public function offsetUnset(mixed $offset): void {}

        /**
         * The ordinal of the first column named `$name` exactly — what `Column`'s factories take.
         *
         * @throws \OutOfBoundsException when no column has that name.
         *
         * @param string $name
         * @return int
         */
        public function ordinal(string $name): int {}
    }

    /**
     * One row of a `Batch`: the batch read across rather than down, for code that builds a value a
     * row at a time.
     *
     * ```php
     * foreach ($reader->rows() as $row) {
     *     $id = $row->get(0);                          // HyperCast's union, as Batch::get()
     *     $name = $row->value(1);                      // the value, or null, as Batch::values()
     *     if ($id instanceof Fault) {
     *         echo "line {$row->line()}: {$id->reason->name} in {$row->raw(0)}\n";
     *     }
     * }
     * ```
     *
     * Each method is the batch's own with this row's index, so a row answers exactly what its batch
     * does. A batch owns what it shows, and so does a row: it stays good after the reader has moved
     * on.
     */
    final class Row {
        /**
         * Made by a batch as it is iterated; not constructible.
         */
        private function __construct() {}

        /**
         * The batch the row is in.
         *
         * @return \HyperTabular\Batch
         */
        public function batch(): \HyperTabular\Batch {}

        /**
         * Why the cell in `$column` did not cast, or null when it did: the row's entry of
         * `Batch::faults()`.
         *
         * @param int $column
         * @return \HyperCast\Fault|null
         */
        public function fault(int $column): ?\HyperCast\Fault {}

        /**
         * The cell in `$column` as HyperCast judged it: `Batch::get()`.
         *
         * @param int $column
         * @return \HyperCast\Success|\HyperCast\Fault
         */
        public function get(int $column): \HyperCast\Success|\HyperCast\Fault {}

        /**
         * The row's place in its batch, from zero.
         *
         * @return int
         */
        public function index(): int {}

        /**
         * Where the row came from: `Batch::line()`.
         *
         * @return int
         */
        public function line(): int {}

        /**
         * The text the cell in `$column` was cast from: `Batch::raw()`.
         *
         * @param int $column
         * @return string
         */
        public function raw(int $column): string {}

        /**
         * The value of the cell in `$column`, or null where it did not cast: the row's entry of
         * `Batch::values()`.
         *
         * @param int $column
         * @return mixed
         */
        public function value(int $column): mixed {}
    }

    /**
     * The rows of a batch (`Batch::getIterator()`), or every row a reader or a sheet has left
     * (`DelimitedReader::rows()`, `Sheet::rows()`), each a `Row`. A reader's rows are read as the
     * iteration reaches them, a batch at a time, forward only: once it has moved past its first
     * row it cannot be rewound, as a generator cannot.
     */
    final class RowIterator implements \Iterator {
        /**
         * Made by a batch or a reader; not constructible.
         */
        private function __construct() {}

        /**
         * The row in hand, or null past the last.
         *
         * @return mixed
         */
        public function current(): mixed {}

        /**
         * The row's key: its index in the batch, or its place in the whole read.
         *
         * @return mixed
         */
        public function key(): mixed {}

        /**
         * Moves to the next row, reading the next batch when this one has run out.
         *
         * @return void
         */
        public function next(): void {}

        /**
         * Starts over: a batch's rows from the first; a reader's rows only before the iteration
         * has moved past the first row.
         *
         * @return void
         */
        public function rewind(): void {}

        /**
         * Whether there is a row in hand.
         *
         * @return bool
         */
        public function valid(): bool {}
    }

    /**
     * The test suite's hook into a workbook's buffers: with `$stingy` set, every buffer a workbook
     * or a sheet is opened with starts with room for one element, so each read proves the core's
     * grow-and-repeat protocol; `$grown` counts the buffers grown — window, arena, cells.
     *
     * @internal
     */
    final class Scratch {
        /**
         * How many times a workbook call grew its window, its arena and its cell table.
         *
         * @var array{int, int, int}|null
         */
        public static $grown;

        /**
         * Whether every workbook buffer starts with room for one element.
         */
        public static $stingy = false;

        /**
         * A test hook; not constructible.
         */
        private function __construct() {}
    }

    /**
     * One sheet of a `Workbook`, read a batch at a time into typed columns — the same `Batch`
     * delimited text gives, its rows numbered by the sheet's own row numbers.
     *
     * As a delimited reader's, the plan can wait for the header: `headerIndex()` looks each name
     * up, and `bind()` declares the plan before the first read. A broken part is a
     * `TabularException`, thrown once every intact row before it has been delivered, and thrown
     * again by every read after.
     */
    final class Sheet {
        /**
         * Made by `Workbook::sheet()`; not constructible.
         */
        private function __construct() {}

        /**
         * Declares the plan, once, before the first read.
         *
         * @throws \LogicException when the sheet already has a plan.
         * @throws \InvalidArgumentException for a plan entry that is not a `Column`.
         *
         * @param array $plan
         * @return void
         */
        public function bind(array $plan): void {}

        /**
         * The header row's names; null when the options say there is no header.
         *
         * @return array|null
         */
        public function header(): ?array {}

        /**
         * The header as a `Header`, to look columns up by name; null when there is none.
         *
         * @return \HyperTabular\Header|null
         */
        public function headerIndex(): ?\HyperTabular\Header {}

        /**
         * Whether the sheet has a plan.
         *
         * @return bool
         */
        public function isBound(): bool {}

        /**
         * The options the sheet is read with.
         *
         * @return \HyperTabular\SheetOptions
         */
        public function options(): \HyperTabular\SheetOptions {}

        /**
         * The plan, as bound; an empty list before one is.
         *
         * @return array
         */
        public function plan(): array {}

        /**
         * The next batch of rows, or null at the end of the sheet.
         *
         * @throws \LogicException when the sheet has no plan.
         * @throws TabularException at a broken part, once every intact row before it has been
         *                          delivered — and again at every read after.
         *
         * @return \HyperTabular\Batch|null
         */
        public function read(): ?\HyperTabular\Batch {}

        /**
         * Every row the sheet has left, a batch at a time, each a `Row` keyed by its place in the
         * read: an iterator that reads as it goes, forward only.
         *
         * @return \Iterator
         */
        public function rows(): \Iterator {}
    }

    /**
     * The binding's own facts.
     */
    final class Tabular {
        /**
         * Holds only static methods; not constructible.
         */
        private function __construct() {}

        /**
         * Whether the binding can read: the hypertabular extension is loaded — this class is part of
         * it — and so is HyperCast's, the judge of every cell. Never throws.
         *
         * @return bool
         */
        public static function isAvailable(): bool {}

        /**
         * The version of the hypertabular extension, `major.minor.patch`.
         *
         * @return string
         */
        public static function nativeVersion(): string {}
    }

    /**
     * Why an input could not be read as rows at all — the kinds of `TabularException`. The values
     * are the core's codes.
     */
    enum TabularFailure: int {
    /**
     * The input ended inside a quoted cell.
     */
      case UnclosedQuote = 1;
    /**
     * A record's cell count disagrees with the first record's.
     */
      case ColumnCount = 2;
    /**
     * A single record is larger than `DelimitedReader::MAX_ROW_BYTES`.
     */
      case RowTooLong = 3;
    /**
     * The workbook's container is not a zip file.
     */
      case NotAZip = 16;
    /**
     * The zip's own structure is broken.
     */
      case Container = 17;
    /**
     * The workbook is encrypted.
     */
      case Encrypted = 18;
    /**
     * A part is compressed by a method other than stored or deflate.
     */
      case Method = 19;
    /**
     * A part the workbook cannot be read without is missing.
     */
      case MissingPart = 20;
    /**
     * A part's XML ends inside a construct.
     */
      case Xml = 21;
    /**
     * A part's bytes are not a deflate stream, or stop before the stream does.
     */
      case Deflate = 22;
    /**
     * The zip is not an XLSX, ODS or Numbers workbook.
     */
      case NotAWorkbook = 23;
    /**
     * A cell names a shared string the table does not have.
     */
      case SharedString = 24;
    /**
     * More text than can be addressed: over 4 GiB in a batch or in the shared strings, or 2
     * GiB in a cell.
     */
      case TooLarge = 25;
    /**
     * A Numbers part is not what the IWA packaging says: found is 1 for a broken chunk or
     * Snappy block, 2 for a malformed message, 3 for an object a reference names that is not
     * there (expected is its identifier), 4 for cell storage of a version the core does not
     * read (expected is the version).
     */
      case Iwa = 26;
    }

    /**
     * An XLSX, ODS or Numbers workbook, opened in memory: its format, its date system, its sheets.
     * A sheet is read through a plan, as delimited text is, into the same `Batch`.
     *
     * ```php
     * $book = Workbook::open('data.xlsx');
     * $sheet = $book->sheet('Data', new SheetOptions());
     * $header = $sheet->headerIndex();
     * $sheet->bind([Column::decimal($header->ordinal('amount'))]);
     * while (($batch = $sheet->read()) !== null) {
     *     $amounts = $batch->values(0);
     * }
     * ```
     *
     * Nothing is sniffed: a cell's text is cast through the column's door, as delimited text is,
     * and a typed cell (a number, a date) is judged the same way. Opening a workbook that is not
     * one — not a zip, encrypted, broken — is a `TabularException`.
     */
    final class Workbook {
        /**
         * Made by `fromString()`, `fromStream()` or `open()`; not constructible.
         */
        private function __construct() {}

        /**
         * The workbook's date system, as HyperCast's `ExcelEpoch`: what `Column::excelSerial()`
         * takes to read its date serials.
         *
         * @return \HyperCast\ExcelEpoch
         */
        public function dateSystem(): \HyperCast\ExcelEpoch {}

        /**
         * The workbook's format.
         *
         * @return \HyperTabular\WorkbookFormat
         */
        public function format(): \HyperTabular\WorkbookFormat {}

        /**
         * A workbook read whole from an open stream, which stays the caller's.
         *
         * @throws \InvalidArgumentException for anything but an open stream.
         * @throws \RuntimeException when reading the stream fails or stops short.
         * @throws TabularException when the bytes are not a workbook this library reads.
         *
         * @param mixed $stream
         * @return \HyperTabular\Workbook
         */
        public static function fromStream(mixed $stream): \HyperTabular\Workbook {}

        /**
         * A workbook from its bytes.
         *
         * @throws TabularException when the bytes are not a workbook this library reads.
         *
         * @param string $bytes
         * @return \HyperTabular\Workbook
         */
        public static function fromString(string $bytes): \HyperTabular\Workbook {}

        /**
         * The workbook in the file at `$path`, read whole with `file_get_contents()`.
         *
         * @throws \RuntimeException when the file cannot be read.
         * @throws TabularException when the bytes are not a workbook this library reads.
         *
         * @param string $path
         * @return \HyperTabular\Workbook
         */
        public static function open(string $path): \HyperTabular\Workbook {}

        /**
         * One sheet, by its place in `sheets()` (an int) or by its name (a string, matched
         * exactly), to read with `$options` and, if it is known, `$plan`.
         *
         * @throws \OutOfRangeException for a place past the last sheet.
         * @throws \OutOfBoundsException for a name no sheet has.
         * @throws TabularException when the sheet's part is broken.
         *
         * @param mixed $which
         * @param \HyperTabular\SheetOptions $options
         * @param array|null $plan
         * @return \HyperTabular\Sheet
         */
        public function sheet(mixed $which, \HyperTabular\SheetOptions $options, ?array $plan = null): \HyperTabular\Sheet {}

        /**
         * The workbook's sheets, in its own order, hidden ones included.
         *
         * @return array
         */
        public function sheets(): array {}
    }

    /**
     * Which kind of workbook a `Workbook` is. The values are the core's codes.
     */
    enum WorkbookFormat: int {
    /**
     * Office Open XML: .xlsx, .xlsm.
     */
      case Xlsx = 1;
    /**
     * OpenDocument: .ods.
     */
      case Ods = 2;
    /**
     * Apple Numbers: .numbers, each table a sheet.
     */
      case Numbers = 3;
    }
}

namespace HyperTabular {
    /**
     * How delimited text is split: the separator, whether quotes are honoured, whether the
     * first record is a header, whether blank lines are skipped.
     */
    final readonly class Dialect {
        public function __construct(
            public string $separator,
            public bool $quoting = true,
            public bool $hasHeader = true,
            public bool $skipBlankLines = true,
        ) {}

        /**
         * Comma-separated, quoted, with a header: one instance per request.
         */
        public static function csv(): \HyperTabular\Dialect {}

        /**
         * Tab-separated, quoted, with a header: one instance per request.
         */
        public static function tsv(): \HyperTabular\Dialect {}

        /**
         * Pipe-separated, quoted, with a header: one instance per request.
         */
        public static function psv(): \HyperTabular\Dialect {}
    }

    /**
     * One output column of a plan: which source column it reads, the door it casts through,
     * and the numeric notation. Built with the factory named after its door.
     */
    final readonly class Column {
        private function __construct(
            public int $ordinal,
            public \HyperTabular\Door $door,
            public \HyperCast\NumFormat $format,
            public int $declared = 0,
        ) {}

        /**
         * A boolean column: HyperCast's boolean lexicon, as `bool`.
         */
        public static function bool(int $ordinal): \HyperTabular\Column {}

        /**
         * A signed 8-bit integer column, as `int`.
         */
        public static function i8(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * A signed 16-bit integer column, as `int`.
         */
        public static function i16(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * A signed 32-bit integer column, as `int`.
         */
        public static function i32(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * A signed 64-bit integer column, as `int`.
         */
        public static function i64(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An unsigned 8-bit integer column, as `int`.
         */
        public static function u8(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An unsigned 16-bit integer column, as `int`.
         */
        public static function u16(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An unsigned 32-bit integer column, as `int`.
         */
        public static function u32(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An unsigned 64-bit integer column, as `int`'s two's-complement bit pattern (sprintf('%u')).
         */
        public static function u64(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An IEEE single column, as `float` (widened losslessly).
         */
        public static function f32(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An IEEE double column, as `float`.
         */
        public static function f64(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * An exact decimal column, as HyperCast's `Decimal` triple.
         */
        public static function decimal(int $ordinal, ?\HyperCast\NumFormat $format = null): \HyperTabular\Column {}

        /**
         * A UUID column, as the lowercase hyphenated string.
         */
        public static function uuid(int $ordinal): \HyperTabular\Column {}

        /**
         * An RFC 3339 instant column, as a UTC DateTimeImmutable.
         */
        public static function timestamp(int $ordinal): \HyperTabular\Column {}

        /**
         * A Unix-epoch column at the declared precision, as a UTC DateTimeImmutable.
         */
        public static function unix(int $ordinal, \HyperCast\UnixPrecision $precision): \HyperTabular\Column {}

        /**
         * An Excel date-serial column under the declared date system, as a UTC DateTimeImmutable.
         */
        public static function excelSerial(int $ordinal, \HyperCast\ExcelEpoch $epoch): \HyperTabular\Column {}

        /**
         * A strict `yyyy-MM-dd` date column, as a DateTimeImmutable at UTC midnight.
         */
        public static function date(int $ordinal): \HyperTabular\Column {}

        /**
         * A separated-date column under the declared field order, as a DateTimeImmutable at UTC midnight.
         */
        public static function dateOrdered(int $ordinal, \HyperCast\DateOrder $order): \HyperTabular\Column {}

        /**
         * A zone-less civil date-time column under the declared field order, on a UTC-labeled DateTimeImmutable.
         */
        public static function datetime(int $ordinal, \HyperCast\DateOrder $order): \HyperTabular\Column {}

        /**
         * A 24-hour time-of-day column, as an exact `int` of nanoseconds since midnight.
         */
        public static function time(int $ordinal): \HyperTabular\Column {}

        /**
         * A duration column, as HyperCast's `Duration` pair.
         */
        public static function duration(int $ordinal): \HyperTabular\Column {}

        /**
         * A text column: the cell's bytes themselves, untrimmed, quotes resolved.
         */
        public static function text(int $ordinal): \HyperTabular\Column {}
    }

    /**
     * One sheet of a workbook, as `Workbook::sheets()` lists it.
     */
    final class SheetInfo {
        public function __construct(
            public readonly string $name,
            public readonly bool $hidden,
            public readonly string $part,
            public readonly int $index,
        ) {}
    }

    /**
     * How a sheet of a `Workbook` is read.
     */
    final class SheetOptions {
        public function __construct(
            public readonly bool $hasHeader = true,
            public readonly bool $skipEmptyRows = true,
            public readonly int $batchRows = \HyperTabular\DelimitedReader::DEFAULT_BATCH_ROWS,
        ) {}
    }

    /**
     * A structural failure: the input is not rows of cells. Never a cell's verdict.
     */
    final class TabularException extends \RuntimeException {
        public function __construct(
            public readonly \HyperTabular\TabularFailure $kind,
            public readonly int $record,
            public readonly int $recordLine,
            public readonly int $byte,
            public readonly int $expected = 0,
            public readonly int $found = 0,
        ) {}
    }
}
