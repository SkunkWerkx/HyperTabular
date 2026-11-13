# HyperTabular for PHP

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell, from `hypertabular`,
a PHP extension installed prebuilt by [PIE](https://github.com/php/pie): no compiler, no Composer
dependency, no FFI.

```sh
pie install skunkwerkx/hypercast
pie install skunkwerkx/hypertabular
```

PHP 8.3 or newer. The whole API is the extension's: the classes below exist once it is loaded,
with no PHP code in your `vendor/`. Every cell is judged by HyperCast's own extension, and every
verdict is HyperCast's own `Success` or `Fault`, so both extensions are installed; they load in
either order.

The examples read this file, saved as `people.csv`; the second row's score does not cast,
so the fault path has something to print:

```csv
id,name,score
1,alice,2.5
2,bob,oops
3,carol,4.75
```

and `people.xlsx`, a workbook with one sheet named `People` holding the same table, header
row first (open `people.csv` in a spreadsheet app, rename the sheet and save it as `.xlsx`).

```php
<?php

use HyperCast\Fault;
use HyperCast\Success;
use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\SheetOptions;
use HyperTabular\Workbook;

// Open first, read the header, then build the plan from its names.
$reader = DelimitedReader::open('people.csv', Dialect::csv());
echo 'header: [' . implode(', ', $reader->header()) . "]\n"; // ['id', 'name', 'score']
$header = $reader->headerIndex(); // the same names, looked up by name
$reader->bind([
    Column::i32($header->ordinal('id')), // an OutOfBoundsException naming a missing column, an AmbiguousColumn a doubled one
    Column::text($header->ordinal('name')),
    Column::f64($header->ordinal('score')),
]);

while (($batch = $reader->read()) !== null) {
    // A column at a time — null where a cell did not cast…
    echo 'ids: [' . implode(', ', $batch->values(0)) . "]\n";
    echo 'faults: ' . count($batch->faults(2)) . "\n"; // that column's, keyed by row

    // …or a row at a time, each cell as HyperCast's union.
    foreach ($batch as $row) {
        $verdict = $row->get(2);
        echo match (true) {
            $verdict instanceof Success => "{$row->value(1)}: {$verdict->value}\n",
            $verdict instanceof Fault => "line {$row->line()}: {$verdict->reason->name} in \"{$row->raw(2)}\"\n",
        };
    }
}
$reader->close();

// A plan known up front is handed straight to the factory.
$plan = [Column::i32(0), Column::text(1), Column::f64(2)];
$reader = DelimitedReader::open('people.csv', Dialect::csv(), $plan);
foreach ($reader->rows() as $row) { // every row, batch after batch
    echo "line {$row->line()}: {$row->value(1)}\n";
}
$reader->close();

// A workbook — from a path, a string or a stream — reads into the same batch.
$stream = fopen('people.xlsx', 'rb');
$book = Workbook::fromStream($stream); // read to its end; the stream stays yours
fclose($stream);
echo 'sheets: [' . implode(', ', array_map(fn ($info) => $info->name, $book->sheets())) . "]\n";
$sheet = $book->sheet('People', new SheetOptions());
$header = $sheet->headerIndex();
$sheet->bind([Column::text($header->ordinal('name')), Column::decimal($header->ordinal('score'))]);
foreach ($sheet->rows() as $row) {
    $verdict = $row->get(1);
    echo match (true) {
        $verdict instanceof Success => "{$row->value(0)}: {$verdict->value}\n",
        $verdict instanceof Fault => "line {$row->line()}: {$verdict->reason->name}\n",
    };
}
```

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose defaults
read as before: lines to pass over, records to discard, what a record of the wrong width is, the
encoding, and what stands for no value.

```php
<?php

use HyperTabular\Column;
use HyperTabular\DelimitedReader;
use HyperTabular\Dialect;
use HyperTabular\Encoding;
use HyperTabular\Range;
use HyperTabular\SheetOptions;
use HyperTabular\ShortRows;

// Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
$dialect = new Dialect(',', skip: 1, shortRows: ShortRows::Pad, encoding: Encoding::Windows1252, tokens: ['NA']);
$data = "Exported by hand\nname,amount\ncaf\xE9,NA\ntea\nmilk,12x4\n";
$plan = [
    Column::text(0),
    Column::f64(1)->withTokens(['NA', '-']), // in place of the dialect's
];
$reader = DelimitedReader::fromString($data, $dialect, $plan);
while (($batch = $reader->read()) !== null) {
    foreach ($batch as $row) {
        $detail = $row->detail(1)?->detail->name ?? '-';
        echo "{$row->value(0)} {$row->get(1)->reason->name} {$row->origin(1)->name} {$detail}\n";
    }
    // What went wrong, and only that: a blank, a token and a padded cell aren't problems.
    foreach ($batch->faults() as $problem) {
        echo "line {$problem->line} column {$problem->sourceColumn} {$problem->reason->name} {$problem->detail->name}\n";
    }
}

// A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
$found = DelimitedReader::suggestString("id,total,day\n1,\"1,234.50\",2026-01-07\n2,12.00,2026-01-08\n", Dialect::csv());
foreach ($found->report as $column) {
    echo "{$column->name} {$column->status->name} {$column->door->name} {$column->matched} of {$column->sampled}\n";
}

// A sheet takes the part of it that's the table; its plan's ordinals count from B here.
$options = new SheetOptions(range: Range::parse('B2:D40'));
echo "{$options->range->firstColumn} {$options->range->lastRow}\n";
```

A `TabularException` of kind `TabularFailure::Encoding` comes after every intact row before it, and its
`byte` alone counts the original bytes: a read's spans are all the decoded text's. A declared encoding
decodes a string too, through `$bufferBytes` at a time, so it isn't read in place. A sheet's `Range` is
parsed by the core, so `Range::parse('A5:F200')` means what it means in every binding, with
`new Range($firstRow, $firstColumn, $lastRow, $lastColumn)`, 1-based and `0` for no limit, for what that
can't say, such as every row from the fifth. A plan's ordinals count from the range's first column,
while `Batch::line()` and a problem's `sourceColumn` stay the sheet's own.

A `Fault` is HyperCast's three fields, and what failed in the cell is beside it:
`$batch->detail($column, $row)` and `$row->detail($column)` are HyperCast's `DetailedFault`, the fault's
reason and span with its `FaultDetail`, or `null` where the cell cast or nothing in it failed (a token,
padding). `$batch->origin($column, $row)` and `$row->origin($column)` say where a cell's verdict came
from: `Origin::Cell`, `Token`, `Padded` or `Uncached`. `$batch->faults()` is a list of `Problem`s, the
row, the line, the plan and source columns, the reason, detail, origin and span, a long row reported
with its `expected` and `found`; `$batch->faults($column)` is still one column's `Fault`s keyed by row.
The list is worked out when it's first asked for and kept. A batch is a copy, so the list and the batch
both stay right after the next read and after the reader has gone; nothing here goes stale. A span
counts bytes, on the verdict and the list alike, so `substr($row->raw(1), $fault->offset,
$fault->length)` is the text that failed.

`notes: false`, on a reader's factory or in `SheetOptions`, drops the detail and the origin, which then
say `null` and `Origin::Cell`: the read costs a little less, and `faults()` can't list a formula with no
cached value. `Column::withDateOptions()` hands a date door HyperCast's `DateOptions` and `MonthNames`,
and on a sheet it and `withTokens()` reach only a text cell; `withTokens([])` says a column has none,
the dialect's included. Every distinct list of tokens, and every distinct list of month names, is kept
for the life of the process, as the Rust API keeps them, and under FPM or any long-running server
that's the worker's life, every request it serves: declare them, don't build a new list per file from
what the files say.

A stream that stalls still hands over every row it has finished: a read asks the stream for more only
when it has no finished row in hand, and each ask is one `fread()`, declared encodings included. What
the stream throws, or an `fread()` that fails, goes to the caller and isn't final: the reader stays
where it was, and the next read asks the stream again. A broken record is final, the same
`TabularException` object from every later read. A stream wrapper's PHP runs in the middle of a read, so a
call it makes back into that reader, or into the `rows()` iterating it, is a `\LogicException` and the
read goes on; the reader's `dialect()`, and any other reader, are there to use.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back
`SuggestStatus::Ambiguous` and read as text until a format is declared or another cell settles them,
which takes in a column of prices at three decimals; `new SuggestOptions(dateOrder: ...)` does the same
for `01/02/2026`. `2,5` beside `2.5` is `SuggestStatus::Unresolved`, and `1,23,456`, Indian grouping, is
text until the column declares Indian grouping, because nothing says its culture.

`DelimitedReader::suggest($path, $dialect, $options)` and `suggestString($utf8, $dialect, $options)`
sample a file (opened as `open()` opens it, stream wrappers included) or a string, and
`$sheet->suggest($options)` a sheet, whatever it has been up to, leaving it where it was.
`new SuggestOptions(0)` means 1 and a negative count is refused; options left out, or declaring only a
format, sample `SuggestOptions::DEFAULT_SAMPLE_ROWS`, a thousand rows, at most a batch. A headerless
file is opened twice, so no suggestion is offered of a stream. A failure among the sampled rows is
thrown, and one past them, a broken record or an `fread()` that returns `false`, only says `truncated`.
What a stream wrapper's own PHP throws is yours, and is thrown wherever it comes, sample or not: an
`\ErrorException` your error handler made of a read's warning included. `columnFormats` and
`columnDateOrders` map a source column's ordinal to its own declaration, ahead of `format` and
`dateOrder`, and a format anywhere in them that the core refuses is an `\InvalidArgumentException`
before anything is opened, whether or not the sample would reach its column. A report's `format` is the
caller's own object where one was declared; compare two by `packed()`.

A sheet's number cell has no notation, so a column of them is a number under any format, where text
holding the same digits asks. A column of `0` and `1` is bool, `007` and an integer past int64 are text,
and a typed date in a sheet is text, for a plan to change by hand. With nothing to choose between, a
resolved number is planned in `,` groups of three, and `SuggestedColumn::$format` names what the plan
uses: a later `1,23,456` is refused.

**To keep every row of a sheet, set `skipEmptyRows: false`.** By default a row whose only cells are
formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at `Origin::Uncached`.

A column given no format is Western, the same as `NumFormat::invariant()`: `1,234,567` is 1234567 and
`1,23,456` is `Malformed`. If a column of Indian numbers comes back refused, declare it, one column at a
time, so an Indian column reads beside a Western one in the same read:

```php
$indian = NumFormat::invariant()->withGrouping(NumFormat::GROUPING_INDIAN);
$either = [
    Column::i32(0, $indian), // 1,23,456 is 123456, and 1,234,567 is Malformed
    Column::i32(1), // 1,234,567 is 1234567, and 1,23,456 is Malformed
];
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat::detect()` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `Column::f64` reads `1,2345` as
1.2345, `12,34` as 12.34 and `0,125` as 0.125. A format holding the retired grouping bit `256` (the
words `351` and `863`) can't be made or unserialized by HyperCast; one forged past both is refused by its
`packed()`, which is an `\InvalidArgumentException` here ("Plan column 1's format cannot be honoured",
HyperCast's exception as its previous) when the plan is bound, before any cell, and in a suggest's
options before it opens anything. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape
and the locales neither policy matches.

## Constraints

A numeric or text column can declare HyperCast's `Constraints`, the OpenAPI 3.2 keywords
(`minimum`, `exclusiveMinimum`, `maximum`, `exclusiveMaximum`, `multipleOf` with `rounding`,
`minLength`, `maxLength`, `enum`, with `trim` and `caseSensitive` for text), through the column
factory's `constraints`. The declaration is for the column's door: `Constraints` for `Door::I32` on a
`Column::u8` is an `InvalidArgumentException`, and a door that takes no constraints has no factory
argument for them. Each cell is cast first, so text that isn't the type is a `Fault` as ever, and then
judged:

```php
use HyperCast\{Checked, Constraints, Door, Fault, Violation};
use HyperTabular\{Column, DelimitedReader, Dialect};

$plan = [
    Column::i32(0, constraints: new Constraints(Door::I32, minimum: 1, maximum: 10)),
    Column::text(1, new Constraints(Door::Text, enum: ['Female', 'Male'])),
];
$batch = DelimitedReader::fromString("qty,sex\n12,MALE\n7,x\n", Dialect::csv(), $plan)->read();
foreach ($batch as $row) {
    $qty = $row->getChecked(0);
    echo match (true) {
        $qty instanceof Checked => $qty->value,
        $qty instanceof Violation => "{$qty->value} breaks {$qty->keywordName()}",
        $qty instanceof Fault => $qty->reason->name,
    };
    // "MALE" matches the entry, and reads as "Male".
    $sex = $row->getChecked(1);
    echo ' ', $sex instanceof Checked ? $sex->value : $sex::class, "\n";
}
foreach ($batch->faults() as $problem) {
    echo "line {$problem->line} column {$problem->sourceColumn} {$problem->keyword->name}\n";
}
```

It prints `12 breaks maximum Male`, `7 HyperCast\Violation`, `line 2 column 0 Maximum` and
`line 3 column 1 Enum`. A violation isn't a fault: the cell cast, so `faults($column)` and a row's
`fault()` leave it out, its value in `values()` and a row's `value()` is kept exactly as cast (never
rounded), and its span is the judged text's bytes. A cell `multipleOf` rounded passes as `Checked`
with `adjusted` set, and a text `enum` match reads as the entry as declared, the caller's own string,
with `enumIndex` naming it. `faults()` lists a violation with its `keyword` (and `keywordCode`) and a
`reason` of `null`. A batch's `get()` and `verdicts()` and a row's `get()` throw a `\LogicException`
on a constrained column, since they would read a violation as passed; `getChecked()`, and
`checkedVerdicts()` for the whole column at once, read an unconstrained column too, where every cell
that cast passes, unadjusted.

## The shape

The core owns no memory and reads no files; the Rust reader underneath `DelimitedReader` and `Sheet`
owns the buffers and grows them, and a batch is copied out of them as it is read, each column made
into PHP values the first time it is asked for.

- **The header first, if you like.** `DelimitedReader::open`, `fromString`, `fromStream`
  and `Workbook::sheet` take the plan, or leave it out (`null`) to read the header and stop.
  `header()` is the names as an array; `headerIndex()` is a `Header` over the same names —
  countable, iterable, indexable — with `ordinal($name)` (exact and case-sensitive: an
  `OutOfBoundsException` naming the column when it is missing, and an `AmbiguousColumn`, an
  `InvalidArgumentException` with `name`, `first` and `second`, when two columns have it, since a
  plan bound to the first would read the wrong column without a word) and `find($name)` (the first
  match, `null` when there's none). `bind($plan)` declares the plan once, before the first read —
  a second `bind` is a `LogicException`, and so is `read()` until a plan is bound; `isBound()` says
  which. `columnCount()` is a record's width. Once a delimited header has been read, a plan column
  that reads at or past its width is refused with an `InvalidArgumentException` naming the
  column: no record has a cell there.
- **One batch class.** `read()` returns a `Batch` — `rows()`, `columns()`, `line($row)`,
  `values($column)`, `faults()`, `verdicts($column)`, `checkedVerdicts($column)`,
  `get($column, $row)`, `getChecked($column, $row)`, `origin($column, $row)`, `detail($column, $row)` and
  `raw($column, $row)` — or `null` once there are no more rows, for
  delimited text and a sheet alike. A batch owns what it shows: it stays good after the reader has
  moved on.
- **Or a row at a time.** A `Batch` iterates its rows as `Row` objects — `index()`,
  `line()`, `get($column)`, `getChecked($column)`, `value($column)`, `fault($column)`,
  `origin($column)`, `detail($column)`, `raw($column)`, each the batch's own answer for that row —
  and `rows()` on a reader or a sheet is a `RowIterator` over every row left, reading batch after
  batch, forward only. A row owns what it shows, as its batch does.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header, and what the
  previous section lists; `SheetOptions` states a sheet's header, whether empty rows are skipped, the
  batch size, its range and the rest; the plan states each column's door and, for numbers, its
  `HyperCast\NumFormat`.
- **Sizes are the core's to judge.** A `$batchRows` or `$bufferBytes` of `0` is an
  `\InvalidArgumentException` saying `batch_rows must be at least 1` (or `buffer_bytes`), and a batch
  past what the core allows says what it asks for, `batch_rows asks for 5377130496 bytes, past the
  limit of 4294967296`, before anything is allocated or a file opened; a negative count is refused
  as no count at all. Left out, a batch is `DelimitedReader::DEFAULT_BATCH_ROWS` rows.
  `new SheetOptions(batchRows: 0)` is made, and refused by `Workbook::sheet()`.
- **No async API.** PHP has no event loop of its own to hand a read to; a stream is read as a
  blocking stream, so under an event loop (Amp, ReactPHP) a read holds the loop while it
  waits, as any blocking `fread` does, and a non-blocking stream with nothing to give yet is
  refused with a `RuntimeException`. A workbook is wholly in memory once it is open.
- **HyperCast is the judge.** `Success`, `Fault`, `DetailedFault`, `FaultDetail`, `CastFailure`,
  `NumFormat`, `Decimal`, `Duration`, `DateOptions`, `MonthNames`, `UnixPrecision`, `DateOrder` and
  `ExcelEpoch` are the `hypercast` extension's own classes. A text cell means exactly what
  `HyperCast\Cast` would say of the same text; a typed workbook cell is converted by the door directly.
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast is
  a `Fault` in its column and the read goes on. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, throws
  `HyperTabular\TabularException`, after every intact row before it — and the same exception
  object again from every read after.

## Install

[PIE](https://github.com/php/pie), the PHP Foundation's extension installer, resolves each
package on Packagist, downloads the zip built for your platform and PHP minor from the GitHub
Release, installs it into the extension directory and enables it. Nothing compiles: the
packages have no source build, so where no zip fits, PIE stops with its own error that no
matching asset exists. `skunkwerkx/hypertabular` requires `ext-hypercast`, so install
HyperCast's first (its [README](https://github.com/SkunkWerkx/HyperCast/blob/master/php/README.md#install)
has what PIE itself needs). `composer require` does not install an extension; to have Composer
check that both are loaded, put `"ext-hypercast": "*"` and `"ext-hypertabular": "*"` in your
project's `require`.

### Platforms and PHP versions

Every platform and minor ships twice: NTS (non-thread-safe), the build every distribution's
`php` CLI and FPM packages ship, and ZTS (thread-safe), the build FrankenPHP, Apache's threaded
MPMs and the thread-safe Windows downloads run. PIE picks the one that matches the PHP it
installs into. 36 zips a release, each one load-checked into the PHP it is named for, and run
through the full suite there, before it is packed:

| Platform | PHP 8.3 | PHP 8.4 | PHP 8.5 | Zip on the release (NTS, ZTS) |
| --- | :---: | :---: | :---: | --- |
| Linux x64, arm64 (glibc 2.34 or newer) | ✅ | ✅ | ✅ | `php_hypertabular-v<version>_php<minor>-{x86_64,arm64}-linux-glibc-{nts,zts}.zip` |
| Linux x64, arm64 (musl: Alpine) | ✅ | ✅ | ✅ | `php_hypertabular-v<version>_php<minor>-{x86_64,arm64}-linux-musl-{nts,zts}.zip` |
| macOS arm64 | ✅ | ✅ | ✅ | `php_hypertabular-v<version>_php<minor>-arm64-darwin-bsdlibc-{nts,zts}.zip` |
| Windows x64 | ✅ | ✅ | ✅ | `php_hypertabular-v<version>-<minor>-{nts,ts}-{vs16,vs17}-x86_64.zip` (VS16 for 8.3, VS17 from 8.4) |

On a thread-safe PHP the extension loads at startup only, from the `extension=hypertabular` line PIE
writes to php.ini (or `docker-php-ext-enable hypertabular` in a Docker image): a `dl()` there stops the
script with a warning naming that line. On an NTS PHP `dl()` loads it as before.

glibc 2.34 means Debian 12, Ubuntu 22.04, RHEL 9, Amazon Linux 2023 or newer. An extension is
tied to one PHP minor, so a zip built for 8.4 is refused by 8.5; PIE picks by the PHP it runs
against. Windows on ARM hardware takes the x64 zip, because PHP itself is an x64 process there.
Intel macOS has no zip: Homebrew bottles no PHP for it any more, and nothing here compiles a PHP
to build against. A 32-bit PHP is not supported.

### Checking it is loaded

Without the extension the classes don't exist, so `extension_loaded('hypertabular')` is the
check for a consumer with a fallback. `HyperTabular\Tabular::isAvailable()` says whether the
binding can read — HyperCast's extension is loaded beside it — and never throws; without
HyperCast's, a call that needs HyperCast (a plan's `Column`, a cell's verdict) throws a
`RuntimeException` naming the extension to install. `Tabular::nativeVersion()` returns the
extension's own `"major.minor.patch"`, the same string as `phpversion('hypertabular')`. `php -m`
lists `hypertabular` when it is loaded.

### Stubs for IDEs and static analysis

An extension's classes have no PHP source for an IDE or an analyser to read, so the repository
carries a stub, [`php/stubs/hypertabular.stub.php`](stubs/hypertabular.stub.php): every class,
method, constant and enum case with its types and doc comment, regenerated from the extension
and compared in CI. PIE installs only the extension, so fetch the stub from the release's tag,
beside HyperCast's:

```sh
curl -LO https://raw.githubusercontent.com/SkunkWerkx/HyperTabular/v<version>/php/stubs/hypertabular.stub.php
```

PhpStorm indexes it like any PHP file in the project; PHPStan needs it under `scanFiles`:

```neon
parameters:
    scanFiles:
        - stubs/hypercast.stub.php
        - stubs/hypertabular.stub.php
```

## Differences from 1.0

1.0 was a Composer package over PHP's FFI extension, requiring HyperCast's Composer package.
The API, defaults, verdicts, exceptions and messages are the same in this extension, except:

- It is installed by PIE, with HyperCast's extension beside it (`skunkwerkx/hypercast` is no
  longer a Composer dependency); ext-ffi is no longer required, nor `ffi.enable` under a web
  SAPI.
- PHP 8.3 is the floor (1.0: 8.2), and Intel macOS has no build (1.0 bundled `osx-x64`).
- `rows()` on a reader or a sheet returns a `HyperTabular\RowIterator`, and a batch's
  `getIterator()` an `\Iterator` of the same class; 1.0 returned `\Generator`s. A reader's
  `RowIterator` is forward only, as a generator was: rewinding it once it has moved past its
  first row throws the same `Exception` a generator throws ("Cannot rewind a generator that was
  already run"). A batch's rewinds.
- `Batch`, `Row` and `Sheet` cannot be constructed (1.0's constructors were public but internal:
  a reader, a batch and `Workbook::sheet()` make them), nor `Tabular`. The readers and what they
  hand out cannot be serialized or cloned. `TabularException::from()`, which took an
  `FFI\CData`, is gone; `new TabularException(...)` is as it was.
- `Workbook::sheet()` declares `$which` as `mixed` and checks it as `int|string` itself, with
  the engine's own `TypeError` wording; reflection shows `mixed` where 1.0's showed `int|string`.
- `Column`'s factories declare `Column` as their return type where 1.0 declared `self`.
- `Tabular::isAvailable()` is whether HyperCast's extension is loaded beside this one, and
  `Tabular::nativeVersion()` never throws.
- A call with more arguments than a method declares is an `ArgumentCountError`; 1.0, as
  userland PHP, ignored the extras. The classes are internal (`ReflectionClass::isInternal()`
  is `true`), and an exception is reported at the caller's file and line.
- 1.0's `Scratch`, `Native`, `Columns` and `NativePlatform` plumbing is gone: a workbook's buffers
  are the Rust reader's own.
- `Header::ordinal()` refuses a name two columns have with an `AmbiguousColumn`, where 1.0 returned
  the first; `find()` still does. `Batch::faults()` with no column is the batch's list of `Problem`s;
  with one, it is 1.0's map. A header name that isn't UTF-8 is a `TabularException` of kind
  `TabularFailure::Encoding`, and a sheet's name is its text with anything that isn't UTF-8
  replaced, as the core reads it.
- A size the core refuses says so in the core's words (`batch_rows must be at least 1`, `batch_rows
  asks for … bytes, past the limit of …`), before a file is opened.
- A stream's failure goes to the caller and isn't final, the next read asking the stream again; a
  stream that hands back nothing before its end is still a `RuntimeException`.

## Verifying provenance

PIE does not check attestations on what it downloads, so verify the zip yourself before you
trust it. Each one is named by the release attestation, signed by this repository's
`release.yml` at the tag, with the release inventory as its predicate:

```sh
gh release download v<version> --repo SkunkWerkx/HyperTabular \
  --pattern 'php_hypertabular-v<version>_php8.5-x86_64-linux-glibc-nts.zip'
gh attestation verify php_hypertabular-v<version>_php8.5-x86_64-linux-glibc-nts.zip \
  --repo SkunkWerkx/HyperTabular --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Each zip also carries build provenance from `hyper-build-php-extension.yml` in
`SkunkWerkx/.github`, the workflow that built, tested and packed it at the release's
preparation commit, so that check needs `--signer-repo` beside `--repo`:

```sh
gh attestation verify <zip> --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
```

Get the signer wrong and `gh` reports a bare `verifying with issuer "sigstore.dev"`, which
reads like a bad signature but is only an identity mismatch. The root README's
[Provenance](../README.md#provenance) section has the whole picture: the inventory, which
artifacts are signed from where, and the gates every byte passes before publication.

## Development

The Composer package Packagist serves is the root [`composer.json`](../composer.json), a
`php-ext` package that PIE reads for its `php-ext` block and its `require`: Packagist requires
the manifest at the top of the git repository it watches. This directory's `composer.json` is
the development project, never published: PHPUnit, PHPBench and PHP_CodeSniffer. To build the
extension from a checkout and run the suite against it (a Rust toolchain and PHP's development
headers, `php-config` on `PATH`), with HyperCast's extension installed by PIE or built in a
HyperCast checkout beside this one:

```sh
git clone https://github.com/SkunkWerkx/HyperTabular
cd HyperTabular/rust
cargo php-ext                  # target/php/release/libhypertabular.so (.dylib on macOS)
cd ../php
composer install
composer test                  # PHPUnit with -d extension= pointing at both builds
composer bench                 # PHPBench, the same way
```

`HYPERTABULAR_PHP_EXTENSION` names another build of this extension for `composer test` and
`composer bench`, and `HYPERCAST_PHP_EXTENSION` a build of HyperCast's; without it, one the PHP
already loads is used, else `../HyperCast/rust/target/php/release/`. The suite replays
`corpus/delimited.json`, through stream buffers starting at one byte, and `corpus/workbook.json`;
a workbook's buffers are the Rust reader's, whose own suite replays them from one element. The
`big-sizes` group asks for batches just past the core's limit and reads a 268,000-column sample:
run it where a mistake is contained (`--exclude-group big-sizes` leaves it out). `HYPERTABULAR_CHECK_STUBS=1 composer test` also regenerates the
stub and compares it with the committed one (`php tools/generate-stubs.php` rewrites it; it
needs the cargo-php version the script names, on Linux or macOS). `vendor/bin/phpcs` checks the
tests, the benchmarks and the tools.

## License

[MIT](../LICENSE)
