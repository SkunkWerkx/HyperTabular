# HyperTabular for PHP

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```
composer require skunkwerkx/hypertabular
```

PHP 8.2 or newer with `ext-ffi` enabled (`ffi.enable=true` for a web server; the CLI's
default, `preload`, allows it on the command line).

```php
use HyperCast\Fault;
use HyperCast\Success;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\SheetOptions;
use HyperTabular\Workbook;

// Open first, read the header, then build the plan from its names.
$reader = DelimitedReader::open('orders.csv', Dialect::csv());
$reader->header();                       // ['id', 'name', 'score']
$header = $reader->headerIndex();        // the same names, looked up by name
$reader->bind([
    Column::i32($header->ordinal('id')), // an OutOfBoundsException naming a missing column
    Column::text($header->ordinal('name')),
    Column::f64($header->ordinal('score')),
]);

while (($batch = $reader->read()) !== null) {
    // A column at a time, decoded in one unpack — null where a cell did not cast…
    $scores = $batch->values(2);
    foreach ($batch->faults(2) as $row => $fault) {
        echo "line {$batch->line($row)}: {$fault->reason->name} in \"{$batch->raw(2, $row)}\"\n";
    }

    // …or a row at a time, each cell as HyperCast's union.
    foreach ($batch as $row) {
        $verdict = $row->get(2);
        echo match (true) {
            $verdict instanceof Success => "{$row->raw(1)}: {$verdict->value}\n",
            $verdict instanceof Fault => "{$verdict->reason->name}\n",
        };
    }
}
$reader->close();

// A plan known up front is handed straight to the factory.
$plan = [Column::i32(0), Column::text(1), Column::f64(2)];
$reader = DelimitedReader::fromString("id,name,score\n1,alice,2.5\n", Dialect::csv(), $plan);
foreach ($reader->rows() as $row) {      // every row, batch after batch
    $row->line();                        // 2
    $row->value(0);                      // 1, or null where the cell did not cast
}

// A workbook — from a path, a string or a stream — reads into the same batch.
$stream = fopen('orders.xlsx', 'rb');
$book = Workbook::fromStream($stream);   // read to its end; the stream stays yours
fclose($stream);
$book->sheets();                         // [SheetInfo { name: 'Orders', hidden: false, … }]
$sheet = $book->sheet('Orders', new SheetOptions());
$sheet->bind([Column::i32($sheet->headerIndex()->ordinal('id'))]);
foreach ($sheet->rows() as $row) {
    // …
}
```

## The shape

The native core owns no memory and reads no files. `DelimitedReader` and `Sheet` allocate
the buffers through ext-ffi — one value array and one verdict array per column, the table
that locates each cell — once, and the core fills them in one native call per batch; a
column then comes out of its buffer in one `unpack()`. The boundary is crossed once per few
thousand rows, not once per cell.

- **The header first, if you like.** `DelimitedReader::open`, `fromString`, `fromStream`
  and `Workbook::sheet` take the plan, or leave it out (`null`) to read the header and stop.
  `header()` is the names as an array; `headerIndex()` is a `Header` over the same names —
  countable, iterable, indexable — with `ordinal($name)` (exact, case-sensitive, first match;
  an `OutOfBoundsException` naming the column when it is missing) and `find($name)` (`null`
  instead). `bind($plan)` declares the plan once, before the first read — a second `bind`
  is a `LogicException`, and so is `read()` until a plan is bound; `isBound()` says which.
  `columnCount()` is a record's width.
- **One batch class.** `read()` returns a `Batch` — `rows()`, `columns()`, `line($row)`,
  `values($column)`, `faults($column)`, `verdicts($column)`, `get($column, $row)` and
  `raw($column, $row)` — or `null` once there are no more rows, for delimited text and a
  sheet alike. A batch owns what it shows: it stays good after the reader has moved on.
- **Or a row at a time.** A `Batch` iterates its rows as `Row` objects — `index()`,
  `line()`, `get($column)`, `value($column)`, `fault($column)`, `raw($column)`, each the
  batch's own answer for that row — and `rows()` on a reader or a sheet is a generator of
  every row left, reading batch after batch. A row owns what it shows, as its batch does.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header, whether empty rows are skipped and the batch size;
  the plan states each column's door and, for numbers, its `HyperCast\NumFormat`.
- **No async API.** PHP has no event loop of its own to hand a read to; a stream is read as a
  blocking stream, and a Fiber-based runtime (Amp, ReactPHP) drives it as it drives any other.
  A workbook is wholly in memory once it is open.
- **HyperCast is the judge.** `Success`, `Fault`, `CastFailure`, `NumFormat`, `Decimal`,
  `Duration`, `UnixPrecision`, `DateOrder` and `ExcelEpoch` are the `skunkwerkx/hypercast`
  package's own. A text cell means exactly what `HyperCast\Cast` would say of the same text;
  a typed workbook cell is converted by the door directly.
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast is
  a `Fault` in its column and the read goes on. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, throws
  `HyperTabular\TabularException`, after every intact row before it.

`HyperTabular\Tabular::isAvailable()` and `Tabular::nativeVersion()` report the loaded core.

## Platforms

The package carries the shared library for Linux (glibc and musl) on x86-64 and arm64,
macOS on Apple silicon and Intel, and Windows on x86-64; PHP on Windows ARM64 runs as an x64
process and loads the x64 library. Packagist installs straight from the git tag, so the
libraries committed under `php/src/native/` are what a consumer loads.

## Verifying provenance

Each committed library was attested when the forge built it and verified again before it
was committed:

```
gh attestation verify src/native/linux-x64/libhypertabular.so --owner SkunkWerkx
```

## License

[MIT](../LICENSE)
