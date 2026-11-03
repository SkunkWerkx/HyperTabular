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
    Column::i32($header->ordinal('id')), // an OutOfBoundsException naming a missing column
    Column::text($header->ordinal('name')),
    Column::f64($header->ordinal('score')),
]);

while (($batch = $reader->read()) !== null) {
    // A column at a time — null where a cell did not cast…
    echo 'ids: [' . implode(', ', $batch->values(0)) . "]\n";
    echo 'faults: ' . count($batch->faults(2)) . "\n";

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

## The shape

The core owns no memory and reads no files. `DelimitedReader` and `Sheet` own the buffers —
one value array and one verdict array per column, the table that locates each cell — allocated
once, and the core fills them in one call per batch; a column is then made into PHP values the
first time it is asked for.

- **The header first, if you like.** `DelimitedReader::open`, `fromString`, `fromStream`
  and `Workbook::sheet` take the plan, or leave it out (`null`) to read the header and stop.
  `header()` is the names as an array; `headerIndex()` is a `Header` over the same names —
  countable, iterable, indexable — with `ordinal($name)` (exact, case-sensitive, first match;
  an `OutOfBoundsException` naming the column when it is missing) and `find($name)` (`null`
  instead). `bind($plan)` declares the plan once, before the first read — a second `bind`
  is a `LogicException`, and so is `read()` until a plan is bound; `isBound()` says which.
  `columnCount()` is a record's width. Once a delimited header has been read, a plan column
  that reads at or past its width is refused with an `InvalidArgumentException` naming the
  column: no record has a cell there.
- **One batch class.** `read()` returns a `Batch` — `rows()`, `columns()`, `line($row)`,
  `values($column)`, `faults($column)`, `verdicts($column)`, `get($column, $row)` and
  `raw($column, $row)` — or `null` once there are no more rows, for delimited text and a
  sheet alike. A batch owns what it shows: it stays good after the reader has moved on.
- **Or a row at a time.** A `Batch` iterates its rows as `Row` objects — `index()`,
  `line()`, `get($column)`, `value($column)`, `fault($column)`, `raw($column)`, each the
  batch's own answer for that row — and `rows()` on a reader or a sheet is a `RowIterator`
  over every row left, reading batch after batch, forward only. A row owns what it shows, as
  its batch does.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header, whether empty rows are skipped and the batch size;
  the plan states each column's door and, for numbers, its `HyperCast\NumFormat`.
- **No async API.** PHP has no event loop of its own to hand a read to; a stream is read as a
  blocking stream, so under an event loop (Amp, ReactPHP) a read holds the loop while it
  waits, as any blocking `fread` does, and a non-blocking stream with nothing to give yet is
  refused with a `RuntimeException`. A workbook is wholly in memory once it is open.
- **HyperCast is the judge.** `Success`, `Fault`, `CastFailure`, `NumFormat`, `Decimal`,
  `Duration`, `UnixPrecision`, `DateOrder` and `ExcelEpoch` are the `hypercast` extension's
  own classes. A text cell means exactly what `HyperCast\Cast` would say of the same text; a
  typed workbook cell is converted by the door directly.
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
- `Scratch`, the suite's hook into a workbook's buffers, is a class of the extension now, still
  internal and not for consumers; 1.0's `Native`, `Columns` and `NativePlatform` plumbing is
  gone.

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
`corpus/delimited.json` and `corpus/workbook.json`, the workbook corpus once more with every
buffer starting at one element. `HYPERTABULAR_CHECK_STUBS=1 composer test` also regenerates the
stub and compares it with the committed one (`php tools/generate-stubs.php` rewrites it; it
needs the cargo-php version the script names, on Linux or macOS). `vendor/bin/phpcs` checks the
tests, the benchmarks and the tools.

## License

[MIT](../LICENSE)
