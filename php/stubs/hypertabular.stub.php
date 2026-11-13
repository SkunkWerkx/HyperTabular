<?php

/**
 * Stubs for the hypertabular extension, for IDEs and static analysis. Generated: don't edit.
 * Regenerate from php/ with `php tools/generate-stubs.php`, which runs
 * `cargo php stubs --stdout` (cargo-php 0.2.2) on the debug build of the extension and
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
     * shows — the arrays the core wrote are copied out of the reader's as it is made — so
     * it stays good after the reader has moved on. A value is the PHP carrier HyperCast's `Cast`
     * returns for the column's door (`Door` lists them).
     */
    final class Batch implements \IteratorAggregate {
        /**
         * Made by a reader or a sheet as it reads; not constructible.
         */
        private function __construct() {}

        /**
         * A column as HyperCast's checked door judges it, one verdict per row: each cell as
         * `getChecked()` reads it. Any column can be read this way, a constrained one and one that
         * declares nothing alike.
         *
         * @param int $column
         * @return array
         */
        public function checkedVerdicts(int $column): array {}

        /**
         * The plan the batch was read through: column `i` of the batch is `columns()[i]`.
         *
         * @return array
         */
        public function columns(): array {}

        /**
         * What failed in the cell at (`$column`, `$row`), as HyperCast's `DetailedFault`: its `Fault`'s
         * reason and span, and the `FaultDetail`. Null where the cell cast, where nothing failed in it
         * (a declared token, padding), and with notes off.
         *
         * @param int $column
         * @param int $row
         * @return \HyperCast\DetailedFault|null
         */
        public function detail(int $column, int $row): ?\HyperCast\DetailedFault {}

        /**
         * With no column, everything wrong in this batch, by row and then by column, a row's own entry
         * first: a list of `Problem`. Listed are cells at `Malformed` or `OutOfRange`, cells that broke
         * a constraint their column declares (with its `keyword`), a workbook's formulas with no cached
         * value, and rows a `LongRows::Report` dialect cut. Blanks, declared
         * tokens, padding and a row cut under `LongRows::Truncate` aren't. With notes off a refused
         * cell has no detail and a formula with no cached value isn't listed. A row the empty-row
         * filter dropped isn't in the batch to be named. Made the first time it is asked for.
         *
         * With a column, that column's cells that did not cast: HyperCast's `Fault` for each, keyed by
         * row, empty when every cell cast.
         *
         * @param int|null $column
         * @return array
         */
        public function faults(?int $column = null): array {}

        /**
         * The cell at (`$column`, `$row`) as HyperCast judged it: its `Success` around the value —
         * the PHP carrier of the column's door (`Door`) — or its `Fault`, whose span indexes the
         * cell's own text (`raw()`). A `\LogicException` for a column that declares constraints,
         * which this would read a violation of as passed: read it with `getChecked()`.
         *
         * @param int $column
         * @param int $row
         * @return \HyperCast\Success|\HyperCast\Fault
         */
        public function get(int $column, int $row): \HyperCast\Success|\HyperCast\Fault {}

        /**
         * The cell at (`$column`, `$row`) as HyperCast's checked door judges it, for a column that
         * declares `Constraints`: `Checked` when it cast and met them (`adjusted` when a `multipleOf`
         * rounding changed the value, and a text `enum` match the entry as declared, with its
         * `enumIndex`), its `Violation` when it cast and broke one (the keyword, the value as cast and
         * the judged text's span), or its `Fault` when it didn't cast. A column that declares none is
         * read too: every cell that cast passes, unadjusted.
         *
         * @param int $column
         * @param int $row
         * @return \HyperCast\Checked|\HyperCast\Fault|\HyperCast\Violation
         */
        public function getChecked(int $column, int $row): \HyperCast\Checked|\HyperCast\Fault|\HyperCast\Violation {}

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
         * Where the verdict of the cell at (`$column`, `$row`) came from: a door, a declared token,
         * padding, or a workbook's formula with no cached value. With notes off a token or a padded
         * cell answers `Origin::Cell`: its verdict is still `Empty`, and nothing was kept of why.
         *
         * @param int $column
         * @param int $row
         * @return \HyperTabular\Origin
         */
        public function origin(int $column, int $row): \HyperTabular\Origin {}

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
         * where the cell did not cast, whatever the reason; `faults()` says why. A cell that broke a
         * constraint its column declares cast, so its value is here as cast, never rounded. Built in
         * one piece the first time it is asked for.
         *
         * @param int $column
         * @return array
         */
        public function values(int $column): array {}

        /**
         * A column as verdicts, one per row: HyperCast's `Success` around the value, or its `Fault`.
         * A `\LogicException` for a column that declares constraints: read it with
         * `checkedVerdicts()`.
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
     * The Rust reader underneath owns every buffer: it casts each plan column in one loop, and `read()`
     * returns a copy of the rows it read as a `Batch`.
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
         * @throws \LogicException when the reader's own stream, mid-read, calls this.
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
         * A reader over an open stream, read a buffer at a time, one `fread()` for as many complete
         * records as a read wants, so a pipe's records aren't held back for more. The stream stays the
         * caller's: the reader never closes it. A stream that fails goes to the caller and isn't final:
         * the next `read()` reads on from where it was.
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
         * @param bool $notes
         * @return \HyperTabular\DelimitedReader
         */
        public static function fromStream(mixed $stream, \HyperTabular\Dialect $dialect, ?array $plan = null, int $batchRows = self::DEFAULT_BATCH_ROWS, int $bufferBytes = self::DEFAULT_BUFFER_BYTES, bool $notes = true): \HyperTabular\DelimitedReader {}

        /**
         * A reader over `$utf8`, held in memory and read in place: `$bufferBytes` matters only under a
         * declared encoding, which decodes ahead of the read.
         *
         * @throws \InvalidArgumentException for a batch or buffer of no rows or bytes or one past what
         *                                   the core allows, a plan entry that is not a `Column`, a
         *                                   separator the core refuses or a plan column past the
         *                                   header's width.
         * @throws TabularException when the header itself is broken.
         *
         * @param string $utf8
         * @param \HyperTabular\Dialect $dialect
         * @param array|null $plan
         * @param int $batchRows
         * @param int $bufferBytes
         * @param bool $notes
         * @return \HyperTabular\DelimitedReader
         */
        public static function fromString(string $utf8, \HyperTabular\Dialect $dialect, ?array $plan = null, int $batchRows = self::DEFAULT_BATCH_ROWS, int $bufferBytes = self::DEFAULT_BUFFER_BYTES, bool $notes = true): \HyperTabular\DelimitedReader {}

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
         * PHP knows — and closed by `close()` or when the reader is destroyed. The arguments are judged
         * before the file is opened: a size the core refuses says so ahead of a missing file.
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
         * @param bool $notes
         * @return \HyperTabular\DelimitedReader
         */
        public static function open(string $path, \HyperTabular\Dialect $dialect, ?array $plan = null, int $batchRows = self::DEFAULT_BATCH_ROWS, int $bufferBytes = self::DEFAULT_BUFFER_BYTES, bool $notes = true): \HyperTabular\DelimitedReader {}

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
         * @throws \LogicException as well when its own stream, mid-read, calls back into the reader.
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

        /**
         * What a sample of the file at `$path`, opened as `open()` opens it, says the columns are: a
         * plan to change or to pass to a read, and why. It starts you off, and it's yours to inspect:
         * nothing is applied, and a read never sniffs.
         *
         * Every format `$options` holds is judged first, before anything is opened. Then what opening
         * the file can throw, and `TabularException` for a failure among the sampled rows. A failure
         * past them isn't met, or, when the stream's `fread()` returns `false`, only says the source was
         * truncated. An exception the stream's own PHP throws is the caller's, and is rethrown wherever
         * it comes. A sample of no header reads the file's first record alone and then opens it again.
         *
         * @throws \InvalidArgumentException for a format the core refuses, or too many columns.
         * @throws \RuntimeException when the file cannot be opened, or reading it fails among the
         *                           sampled rows.
         * @throws \Throwable whatever the stream's own PHP throws.
         * @throws TabularException at a broken record among the sampled rows.
         *
         * @param string $path
         * @param \HyperTabular\Dialect $dialect
         * @param \HyperTabular\SuggestOptions|null $options
         * @return \HyperTabular\Suggestion
         */
        public static function suggest(string $path, \HyperTabular\Dialect $dialect, ?\HyperTabular\SuggestOptions $options = null): \HyperTabular\Suggestion {}

        /**
         * `suggest()`, over a string.
         *
         * @throws \InvalidArgumentException for a format the core refuses, or too many columns.
         * @throws TabularException at a broken record among the sampled rows.
         *
         * @param string $utf8
         * @param \HyperTabular\Dialect $dialect
         * @param \HyperTabular\SuggestOptions|null $options
         * @return \HyperTabular\Suggestion
         */
        public static function suggestString(string $utf8, \HyperTabular\Dialect $dialect, ?\HyperTabular\SuggestOptions $options = null): \HyperTabular\Suggestion {}
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
     * What a delimited file's bytes are, declared and never sniffed. A workbook declares its own.
     */
    enum Encoding: int {
    /**
     * UTF-8, read in place. A cell that isn't valid is that cell's `Malformed` verdict.
     */
      case Utf8 = 0;
    /**
     * Windows-1252. The five bytes the code page leaves undefined are their C1 controls, as WHATWG
     * has them. `EF BB BF` is three characters here, not a byte-order mark.
     */
      case Windows1252 = 1;
    /**
     * Latin-1 (ISO-8859-1): every byte the code point of its value.
     */
      case Latin1 = 2;
    /**
     * UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or half a code
     * unit at the end of the input, is `TabularFailure::Encoding`.
     */
      case Utf16Le = 3;
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
     * A name is matched exactly, byte for byte. `ordinal()` is strict: a name no column has is an
     * `\OutOfBoundsException`, and one that two columns have is an `AmbiguousColumn` naming both, because
     * a plan built from names mustn't silently read the wrong column. `find()` is the optional lookup:
     * it can't carry an error and its caller asked for the first, so it keeps the first match, and a
     * plan by position reads either. A header is read-only: it counts, iterates and indexes as the list
     * of its names, and serializes to JSON as that list.
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
         * The ordinal of the column named `$name` exactly — what `Column`'s factories take.
         *
         * @throws \OutOfBoundsException when no column has that name.
         * @throws AmbiguousColumn when two columns have it: `find()` keeps the first, and a plan can
         *                         name either by position.
         *
         * @param string $name
         * @return int
         */
        public function ordinal(string $name): int {}
    }

    /**
     * What a record with more cells than the width is.
     */
    enum LongRows: int {
    /**
     * A structural failure.
     */
      case Reject = 0;
    /**
     * A row cut to the width, and listed by `Batch::faults()` with the width it had.
     */
      case Report = 1;
    /**
     * A row cut to the width, and nothing said.
     */
      case Truncate = 2;
    }

    /**
     * Where a cell's verdict came from: `Batch::origin()`.
     */
    enum Origin: int {
    /**
     * A door judged the cell's own text.
     */
      case Cell = 0;
    /**
     * The cell's exact unquoted bytes are a declared token, so it is `Empty` and no door ran.
     */
      case Token = 1;
    /**
     * The row has no such cell: padding for a short row, or an ordinal past the row's end.
     */
      case Padded = 2;
    /**
     * A workbook's formula cell with no cached value. It is `Empty` and no door ran.
     */
      case Uncached = 3;
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
         * What failed in the cell in `$column`: `Batch::detail()`.
         *
         * @param int $column
         * @return \HyperCast\DetailedFault|null
         */
        public function detail(int $column): ?\HyperCast\DetailedFault {}

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
         * The cell in `$column` as HyperCast's checked door judged it: `Batch::getChecked()`.
         *
         * @param int $column
         * @return \HyperCast\Checked|\HyperCast\Fault|\HyperCast\Violation
         */
        public function getChecked(int $column): \HyperCast\Checked|\HyperCast\Fault|\HyperCast\Violation {}

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
         * Where the verdict of the cell in `$column` came from: `Batch::origin()`.
         *
         * @param int $column
         * @return \HyperTabular\Origin
         */
        public function origin(int $column): \HyperTabular\Origin {}

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

        /**
         * What a sample of the sheet says its columns are: a plan to change or to pass to a read, and
         * why. It starts you off, and it's yours to inspect: nothing is applied, and a read never
         * sniffs. The sample reads the sheet from its start, whatever this sheet has read, with notes
         * on, and takes one batch of this sheet's size at most. A date column is text: nothing in a
         * sheet's cells says which order a date string is in unless it's declared.
         *
         * @throws \InvalidArgumentException for a format the core refuses, or too many columns.
         * @throws TabularException at a broken part among the sampled rows.
         *
         * @param \HyperTabular\SuggestOptions|null $options
         * @return \HyperTabular\Suggestion
         */
        public function suggest(?\HyperTabular\SuggestOptions $options = null): \HyperTabular\Suggestion {}
    }

    /**
     * What a record with fewer cells than the width is.
     */
    enum ShortRows: int {
    /**
     * A structural failure.
     */
      case Reject = 0;
    /**
     * A row whose missing cells are `Empty`, at `Origin::Padded`.
     */
      case Pad = 1;
    }

    /**
     * What a sample says of a column.
     */
    enum SuggestStatus: int {
    /**
     * One door and one declaration take every cell, and the plan reads the column through them.
     */
      case Resolved = 1;
    /**
     * Every cell is read, but the sample can't say which declaration it's written in, as `1,234`
     * can't: `SuggestedColumn::$candidates` are the choices. The plan reads it as text.
     */
      case Ambiguous = 2;
    /**
     * There's nothing to go on, or a cell no door took beside cells one did. The plan reads it as
     * text.
     */
      case Unresolved = 3;
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
    /**
     * Text that isn't valid UTF-8 where no cell's verdict can carry it, a header name; or a unit
     * that a dialect's declared `Encoding` has no character for, whose `byte` alone counts the
     * original input rather than the decoded text.
     */
      case Encoding = 27;
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
     * How delimited text is split: the separator, whether quotes are honoured, whether a
     * record is a header and which, whether blank lines are skipped, how many lines come
     * before anything is read, what a short or long record is, the text's encoding, and the
     * texts every column reads as an empty cell.
     */
    final readonly class Dialect {
        public function __construct(
            public string $separator,
            public bool $quoting = true,
            public bool $hasHeader = true,
            public bool $skipBlankLines = true,
            public int $skip = 0,
            public int $headerRow = 0,
            public \HyperTabular\ShortRows $shortRows = \HyperTabular\ShortRows::Reject,
            public \HyperTabular\LongRows $longRows = \HyperTabular\LongRows::Reject,
            public \HyperTabular\Encoding $encoding = \HyperTabular\Encoding::Utf8,
            public array $tokens = [],
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
     * the numeric notation, and the HyperCast `Constraints` it declares, if any. Built with the
     * factory named after its door.
     */
    final readonly class Column {
        private function __construct(
            public int $ordinal,
            public \HyperTabular\Door $door,
            public \HyperCast\NumFormat $format,
            public int $declared = 0,
            public ?array $tokens = null,
            public ?\HyperCast\DateOptions $dateOptions = null,
            public ?\HyperCast\MonthNames $months = null,
            public ?\HyperCast\Constraints $constraints = null,
        ) {}

        /**
         * This column, reading each of `$tokens` (and only those) as an empty cell, in place
         * of the dialect's or the sheet's.
         */
        public function withTokens(array $tokens): \HyperTabular\Column {}

        /**
         * This column, reading its dates with `$options` and, when given, `$months`.
         */
        public function withDateOptions(
            \HyperCast\DateOptions $options,
            ?\HyperCast\MonthNames $months = null,
        ): \HyperTabular\Column {}

        /**
         * A boolean column: HyperCast's boolean lexicon, as `bool`.
         */
        public static function bool(int $ordinal): \HyperTabular\Column {}

        /**
         * A signed 8-bit integer column, as `int`.
         */
        public static function i8(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * A signed 16-bit integer column, as `int`.
         */
        public static function i16(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * A signed 32-bit integer column, as `int`.
         */
        public static function i32(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * A signed 64-bit integer column, as `int`.
         */
        public static function i64(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An unsigned 8-bit integer column, as `int`.
         */
        public static function u8(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An unsigned 16-bit integer column, as `int`.
         */
        public static function u16(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An unsigned 32-bit integer column, as `int`.
         */
        public static function u32(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An unsigned 64-bit integer column, as `int`'s two's-complement bit pattern (sprintf('%u')).
         */
        public static function u64(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An IEEE single column, as `float` (widened losslessly).
         */
        public static function f32(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An IEEE double column, as `float`.
         */
        public static function f64(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

        /**
         * An exact decimal column, as HyperCast's `Decimal` triple.
         */
        public static function decimal(
            int $ordinal,
            ?\HyperCast\NumFormat $format = null,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}

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
        public static function text(
            int $ordinal,
            ?\HyperCast\Constraints $constraints = null,
        ): \HyperTabular\Column {}
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
            public readonly int $skip = 0,
            public readonly int $headerRow = 0,
            public readonly ?\HyperTabular\Range $range = null,
            public readonly array $tokens = [],
            public readonly bool $notes = true,
        ) {}
    }

    /**
     * A rectangle of a sheet, its corners 1-based and inclusive, 0 for no limit on a side.
     */
    final readonly class Range {
        public function __construct(
            public int $firstRow = 0,
            public int $firstColumn = 0,
            public int $lastRow = 0,
            public int $lastColumn = 0,
        ) {}

        /**
         * A range in A1 notation: `B2:D10`, `B:D`, `2:10`, or one cell.
         */
        public static function parse(string $text): \HyperTabular\Range {}
    }

    /**
     * One entry of a batch's faults: a cell that did not cast, a cell that broke a constraint its
     * column declares, or a record of the wrong width.
     */
    final readonly class Problem {
        public function __construct(
            public int $row,
            public int $line,
            public ?int $planColumn,
            public ?int $sourceColumn,
            public ?\HyperCast\CastFailure $reason,
            public ?\HyperCast\FaultDetail $detail,
            public int $detailCode,
            public \HyperTabular\Origin $origin,
            public int $offset,
            public int $length,
            public int $expected,
            public int $found,
            public ?\HyperCast\Keyword $keyword = null,
            public int $keywordCode = 0,
        ) {}
    }

    /**
     * What a suggestion samples and what the caller has already declared.
     */
    final readonly class SuggestOptions {
        public const DEFAULT_SAMPLE_ROWS = 1000;

        public function __construct(
            public int $sampleRows = self::DEFAULT_SAMPLE_ROWS,
            public ?\HyperCast\NumFormat $format = null,
            public array $columnFormats = [],
            public ?\HyperCast\DateOrder $dateOrder = null,
            public array $columnDateOrders = [],
        ) {}
    }

    /**
     * A suggested plan and the report that says how each column was chosen.
     */
    final readonly class Suggestion {
        public function __construct(
            public array $plan,
            public array $report,
            public bool $truncated,
        ) {}
    }

    /**
     * How a suggestion chose one column.
     */
    final readonly class SuggestedColumn {
        public function __construct(
            public int $ordinal,
            public ?string $name,
            public \HyperTabular\Door $door,
            public mixed $declared,
            public ?\HyperCast\NumFormat $format,
            public array $candidates,
            public \HyperTabular\SuggestStatus $status,
            public int $matched,
            public int $sampled,
        ) {}
    }

    /**
     * A header name two columns have, refused by the strict lookup.
     */
    final class AmbiguousColumn extends \InvalidArgumentException {
        public function __construct(
            public readonly string $name,
            public readonly int $first,
            public readonly int $second,
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
