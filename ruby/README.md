# hypertabular

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```sh
gem install hypertabular
```

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

```ruby
require "hypertabular"

HyperTabular::DelimitedReader.open("people.csv", HyperTabular::Dialect::CSV) do |reader|
  # The header is read first; the plan is built from where its names are.
  header = reader.header # => ["id", "name", "score"]
  puts "header: #{header.inspect}"
  reader.bind([
    HyperTabular::Column.i32(header.ordinal("id")),
    HyperTabular::Column.text(header.ordinal("name")),
    HyperTabular::Column.f64(header.ordinal("score"))
  ])

  while (batch = reader.read)
    # A column at a time, decoded in one pass — nil where a cell did not cast…
    names = batch.values(1)
    puts "ids: #{batch.values(0).inspect}"
    puts "faults: #{batch.verdicts(2).count { |verdict| verdict.is_a?(HyperCast::Fault) }}"

    # …or a cell at a time, as HyperCast's union.
    batch.rows.times do |row|
      case batch.get(2, row)
      in HyperCast::Success(value:)
        puts "#{names[row]}: #{value}"
      in HyperCast::Fault(reason:)
        puts "line #{batch.line(row)}: #{reason} in #{batch.raw(2, row).inspect}"
      end
    end
  end
end

# Or a row at a time: a reader, a sheet and a batch are Enumerable over Rows. A plan known
# up front goes straight to the factory, bound before the header is read.
plan = [HyperTabular::Column.i32(0), HyperTabular::Column.text(1), HyperTabular::Column.f64(2)]
HyperTabular::DelimitedReader.open("people.csv", HyperTabular::Dialect::CSV, plan) do |reader|
  reader.each { |row| puts "line #{row.line}: #{row.value(1)}" }
end

# A workbook reads into the same batch — from a path, a String or an IO.
book = HyperTabular::Workbook.open("people.xlsx")
puts "sheets: #{book.sheets.map(&:name).inspect}" # SheetInfo name="People", hidden=false
sheet = book.sheet("People", HyperTabular::SheetOptions::DEFAULT)
sheet.bind([
  HyperTabular::Column.text(sheet.header.ordinal("name")),
  HyperTabular::Column.decimal(sheet.header.ordinal("score"))
])
sheet.each do |row|
  case row.get(1)
  in HyperCast::Success(value:)
    puts "#{row.value(0)}: #{value}"
  in HyperCast::Fault(reason:)
    puts "line #{row.line}: #{reason}"
  end
end
```

A plan known up front can be handed over at once —
`DelimitedReader.open(path, dialect, plan)`, `book.sheet(which, options, plan)` — which binds it
before the header is read.

- **The header** (`reader.header`, `sheet.header`) is a `HyperTabular::Header`: a frozen
  Array of frozen names, which also says where a name is. `header.ordinal(name)` is the column
  with exactly that name — case and spaces included — and a `KeyError` naming it when there is
  none (or, with a block, the block's value, as `Hash#fetch` does). A name two columns share is
  a `HyperTabular::AmbiguousColumn`, an `ArgumentError` with `name`, `first` and `second`, whose
  message names both positions, because a plan built from names mustn't read the wrong column:
  say which by position. `header.find_ordinal(name)` answers the first match, or `nil`.
- **Binding** happens once, before the first read: `bind` again raises `RuntimeError`, and so
  does a `read` before `bind` — which `bind` then cures. A plan that is not Columns is an
  `ArgumentError` and leaves the reader unbound; so is, once a delimited header has fixed
  the width every record has, a column at or past it, named in the message (a plan handed
  to the constructor is held to the header as soon as it is read). Without a header such a
  column reads as empty, and a plan reaching past 256 source columns reads shorter batches,
  as many rows as about 8 MiB of cell table holds. `bound?` says which it is. A `nil` plan
  means "bind later"; `[]` is an empty plan, bound at once.
  `batch_rows` sizes the arrays a bound plan reads into. The core judges them before anything is
  allocated: a batch asking for more than 4 GiB, and a `batch_rows` or `buffer_bytes` of 0, are an
  `ArgumentError` from the core, at open or at `bind`. Below that they're asked for zeroed and aren't
  written to until a row reaches them, so a large batch costs what the source holds, and one no memory
  holds is a `NoMemoryError`. Through Fiddle on 64-bit Windows, one array stops at 2 GiB. A delimited
  plan of no columns has no arrays, and reads at any `batch_rows` up to 2^64. `suggest` is never refused
  for the size of its own batches: the core says what fits and the sampler reads as many as it takes. A
  row that alone is past the limit is an `ArgumentError` naming its width.
  `reader.column_count` is the header's count, or the first record's once it has been read.
- **Rows.** `batch.each`, `reader.each` and `sheet.each` yield a `HyperTabular::Row` —
  `index`, `line`, `get(column)` (also `[]` and `verdict`), `value(column)`, `raw(column)`,
  `origin(column)`, `to_a` — each answering what the batch's own accessor does for that row. A batch is a copy,
  so a row stays good after the read has moved on. A Row deconstructs as its verdicts, for
  `case row in [HyperCast::Success(value: id), *]`. `reader.each_row` yields each row as
  a frozen Array of verdicts, and `sheet.each_batch` walks a sheet's batches.
- **A workbook from an IO.** `Workbook.new(io)` reads anything with `#read` to its end into a
  String the workbook owns, as soon as it is opened; `close_source: true` closes the IO once it
  has been read. The kind of workbook is read from its bytes, never from a name.

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose defaults
read as before: lines to pass over, records to discard, what a record of the wrong width is, the
encoding, and what stands for no value.

```ruby
require "hypertabular"

# Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
dialect = HyperTabular::Dialect.new(separator: ",", skip: 1, short_rows: :pad, encoding: :windows_1252,
                                    tokens: ["NA"])
text = "Exported by hand\nname,amount\ncaf\xE9,NA\ntea\nmilk,12x4\n".b
plan = [
  HyperTabular::Column.text(0),
  HyperTabular::Column.f64(1).with_tokens(%w[NA -]) # in place of the dialect's
]
reader = HyperTabular::DelimitedReader.new(text, dialect, plan)
while (batch = reader.read)
  batch.each do |row|
    cell = row.get(1)
    puts "#{row.value(0)} #{cell.reason} #{row.origin(1)} #{cell.detail}"
  end
  # What went wrong, and only that: a blank, a token and a padded cell aren't problems.
  batch.faults.each do |problem|
    puts "line #{problem.line} column #{problem.source_column} #{problem.reason} #{problem.detail}"
  end
end

# A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
found = HyperTabular::DelimitedReader.suggest_string(%(id,total,day\n1,"1,234.50",2026-01-07\n2,12.00,2026-01-08\n),
                                                     HyperTabular::Dialect::CSV)
found.report.each do |column|
  puts "#{column.name} #{column.status} #{column.door} #{column.matched} of #{column.sampled}"
end

# A sheet takes the part of it that's the table; its plan's ordinals count from B here.
cut = HyperTabular::Range.parse("B2:D40")
options = HyperTabular::SheetOptions.new(range: cut)
puts "#{options.range.first_column} #{options.range.last_row}"
```

A `TabularError` of kind `:encoding` comes after every intact row before it, and its `byte` alone
counts the original bytes: a read's spans are all the decoded text's. A declared encoding decodes a
String too, so it isn't read in place. A sheet's `Range` is parsed by the core, so
`Range.parse("A5:F200")` means what it means in every binding, with `Range.new(first_row:,
first_column:, last_row:, last_column:)`, 1-based and `0` for no limit, for what that can't say, such
as every row from the fifth. A plan's ordinals count from the range's first column, while
`Batch#line` and a problem's `source_column` stay the sheet's own.

A `Fault` carries HyperCast's `detail`, and `batch.origin(column, row)` and `row.origin(column)` say
where a cell's verdict came from: `:cell`, `:token`, `:padded` or `:uncached`. `batch.faults` is a
frozen Array of `HyperTabular::Problem`s, the row, the line, the plan and source columns, the reason,
detail, origin and span, a long row reported with its `expected` and `found`. It's worked out when
it's first asked for and kept. A batch is a copy, so the list and the batch both stay right after the
next read and after the reader has gone; nothing here goes stale. A span counts characters, as a
`Fault`'s does, on the verdict and the list alike, so `raw[problem.offset, problem.length]` slices it.

`notes: false`, on a reader or in `SheetOptions`, drops `detail` and `origin`, which then say `nil` and
`:cell`: the read costs a little less, and `faults` can't list a formula with no cached value.
`Column#with_date_options` hands a date door HyperCast's `DateOptions` and `MonthNames`, and on a sheet
it and `with_tokens` reach only a text cell; `with_tokens([])` says a column has none, the dialect's
included. Tokens and month names last as long as the read that holds them: this binding keeps nothing
for the life of the process, on either backend.

A stream that stalls still hands over every row it has finished: a read asks the IO for more only when
it has no finished row in hand, and each ask is one `#readpartial` or `#read`, declared encodings
included. Whatever the IO's read raises goes to the caller and is final, as in Rust and Go, a
transient `IOError` or `SystemCallError` (`Errno::EAGAIN`, `Errno::ETIMEDOUT`) included: Ruby can't
tell one from an exception that landed in the read (below), which may have taken bytes with it, so
the reader isn't resumable after it. Every later read raises `RuntimeError` naming it; open the
source again to go on. A broken record is final too, the same failure on every later read.

A reader or a sheet is called into once at a time. A call back into it from the IO it reads or from
a plan's `each` while it reads or binds (`read`, `each`, `bind`, `close`, a sheet's `suggest`)
raises `RuntimeError`, "the reader is in the middle of a call; nothing that call runs can call back
into it" ("the sheet …" for a sheet), and the call it came from goes on as it was. A call from your
own block, between batches, is yours to make. An exception that lands mid-read, such as
`Timeout.timeout`, `Thread#raise` or Ctrl-C's `Interrupt`, waits until the core and the reader agree
again, so no row is lost or numbered wrong inside the read: a batch it lands on is handed over by
the next `read`, with its own lines. The one place it doesn't wait is the IO's own read while that
blocks, so a timeout still breaks a read waiting on a pipe. Ruby may drop bytes it had already read
from the IO when an exception lands there, so that read raises the exception and every later read
(`each` and the rest included) raises `RuntimeError` naming it: "the reader can't read on: Interrupt
(…) was raised while it was reading its IO, and what that read had taken from the IO may be lost". Once
the IO has answered, the exception waits until its answer is held. Anything raised inside the reader
once the core has read past a batch, such as a `NoMemoryError` while the batch is copied, makes
every later read raise `RuntimeError` naming it. One gap no Ruby method can close is the caller's
own: an exception that lands after `read` has let go of its batch and before the caller holds it
loses that batch. Its rows are missing, never wrong, and the lines `Batch#line` gives the next batch
skip them. A caller that can't lose one holds such exceptions back itself around the read and
whatever keeps its batch, as in
`Thread.handle_interrupt(Object => :never) { batches << reader.read }`; the read hands the batch
over whole, and the exception lands after.

A walk through the rows (`each`, `each_row`, a sheet's `each` and `each_batch`) left part-way
through a batch, by `break`, `first`, `take` or an exception from the block, leaves the rest of that
batch behind, as the Rust core and Python's `reader.rows()` do: the next walk or `read` starts at
the next batch, and the lines `Batch#line` gives show the skip. An Enumerator from `each` without a
block, kept and advanced with `next`, is one walk and goes on where it was.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back `:ambiguous` and
read as text until a format is declared or another cell settles them, which takes in a column of prices
at three decimals; `SuggestOptions.new(date_order: ...)` does the same for `01/02/2026`. `2,5` beside
`2.5` is `:unresolved`, and `1,23,456`, Indian grouping, is text until the column declares Indian
grouping, because nothing says its culture.

`DelimitedReader.suggest(path, dialect, options)` and `suggest_string(text, dialect, options)` sample a
file or a String, and `sheet.suggest(options)` a sheet, whatever it has been up to, leaving it where it
was. `SuggestOptions.new(sample_rows: 0)` means 1 and a negative count is refused; options that declare
only a format still sample a thousand rows, at most a batch. A headerless file is opened twice, so no
suggestion is offered of a stream. A failure among the sampled rows is raised, and one past them only
says `truncated`. `SuggestOptions`' `column_formats` and `column_date_orders` take a Hash or an Array
of pairs, the last for a column winning, and a format anywhere in them that the core refuses is an
`ArgumentError` before anything is opened, whether or not the sample would reach its column. Compare
two formats with `==`: the policy counts.

A sheet's number cell has no notation, so a column of them is a number under any format, where text
holding the same digits asks. A column of `0` and `1` is bool, `007` and an integer past int64 are text,
and a typed date in a sheet is text, for a plan to change by hand. With nothing to choose between, a
resolved number is planned in `,` groups of three, and `SuggestedColumn#format` names what the plan
uses: a later `1,23,456` is refused.

**To keep every row of a sheet, set `skip_empty_rows: false`.** By default a row whose only cells are
formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at origin `:uncached`.

A column given no format is Western, the same as `HyperCast::NumFormat::INVARIANT`: `1,234,567` is
1234567 and `1,23,456` is `:malformed`. If a column of Indian numbers comes back refused, declare it,
one column at a time, so an Indian column reads beside a Western one in the same read:

```ruby
indian = HyperCast::NumFormat::INVARIANT.with(grouping: :indian)
either = [
  HyperTabular::Column.i32(0, indian), # 1,23,456 is 123456, and 1,234,567 is :malformed
  HyperTabular::Column.i32(1)          # 1,234,567 is 1234567, and 1,23,456 is :malformed
]
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `HyperCast::NumFormat::DETECT` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `Column.f64` reads `1,2345` as
1.2345, `12,34` as 12.34 and `0,125` as 0.125. A format holding the retired grouping bit `256` (the
words `351` and `863`), which only a 1.0 Marshal payload or a hand-packed format can hold, raises
`ArgumentError` when the plan is bound, before any cell, and in a suggest's options before it opens
anything. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape
and the locales neither policy matches.

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — one value array and one verdict array per column, the
table that locates each cell — once, and reuse them for every batch. The core fills them in one
native call per batch, and a column comes out of its buffer in one `String#unpack`: the
boundary is crossed once per few thousand rows, not once per cell.

- **One batch class.** `read` returns a `HyperTabular::Batch` — `rows`, `columns`,
  `line(row)`, `values(column)`, `verdicts(column)`, `get(column, row)`, `raw(column,
  row)`, `origin(column, row)`, `faults`, and a `Row` for each row through `each` — or `nil` once there are no more rows,
  for delimited text and a sheet alike. A batch owns what it shows: it stays good after the
  reader has moved on.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and what
  [a messier file](#a-messier-file) needs; `SheetOptions` states a sheet's header, whether empty rows
  are skipped, the batch size and the rest of that; the plan states each column's door and, for
  numbers, its `HyperCast::NumFormat`. A suggestion is a plan to inspect, and nothing reads through
  one until it's handed over.
- **HyperCast is the judge.** `HyperCast::Success`, `HyperCast::Fault`,
  `HyperCast::NumFormat`, `HyperCast::Decimal` and the declared options (`:milliseconds`,
  `:day_month_year`, `:y1904`) are the `hypercast` gem's own. A text cell means exactly
  what HyperCast's door would say of the same text, a typed workbook cell is converted by
  the door directly, and either comes back as the Ruby type that gem returns for it.
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on. A record of the wrong width, input that
  ends inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `HyperTabular::TabularError` carrying `kind`, `record`, `line`, `byte`, `expected` and
  `found`, raised after every intact row before it.
- **The text of any cell is to hand.** `batch.raw(column, row)` is what the cell was cast
  from, whatever its door and verdict, and a `Fault`'s span indexes it in the units
  `String#[]` slices by, as HyperCast's gem does: `raw[fault.offset, fault.length]` is the
  offending text.
- **A String is read in place**, unless its dialect declares an encoding. An IO is read forward only, through a buffer that grows
  when a record does not fit it.
- **No async API, and none needed.** An IO is read with `#readpartial` or `#read`, which yield
  to the Fiber scheduler (the `async` gem's, or any other) instead of blocking, and a task
  cancelled in one is interrupted there; the core's work between reads is on memory.

## The doors

One factory per door, named as HyperCast names them.

| Factory | Value |
|---|---|
| `Column.bool(n)` | `true` / `false` |
| `Column.i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64(n, format = INVARIANT)` | `Integer` |
| `Column.f32` `f64(n, format = INVARIANT)` | `Float` |
| `Column.decimal(n, format = INVARIANT)` | `HyperCast::Decimal`, exact |
| `Column.uuid(n)` | lowercase hyphenated `String` |
| `Column.timestamp(n)` | UTC `Time`, nanosecond fidelity |
| `Column.unix(n, :seconds / :milliseconds / :microseconds / :nanoseconds)` | UTC `Time` |
| `Column.excel_serial(n, :y1900 / :y1904)` | UTC `Time` |
| `Column.date(n)` | `Date`, strict `yyyy-MM-dd` |
| `Column.date(n, order)` / `Column.date_ordered(n, order)` | `Date`, under `:year_month_day`, `:month_day_year` or `:day_month_year` |
| `Column.datetime(n, order)` | `DateTime`, zone-less |
| `Column.time(n)` | `Integer` nanoseconds since midnight |
| `Column.duration(n)` | `Rational` seconds |
| `Column.text(n)` | UTF-8 `String`, untrimmed |

## Backends

| `HyperTabular::BACKEND` | What runs | Chosen when |
|---|---|---|
| `:native` | the core linked into a Magnus extension | a precompiled platform gem is installed — each carries an extension for Ruby 3.4 and 4.0 — or the `hypertabular-wasm` gem is linked into a ruby.wasm interpreter; see [Ruby in the browser](#ruby-in-the-browser) |
| `:fiddle` | `libhypertabular` for this platform, `dlopen`ed through Fiddle | no extension loads: the universal gem, on a Ruby or platform no platform gem covers (Intel macOS, Ruby 3.3, Ruby 4.1 and later) |

Selection happens once, at `require`, in that order. Both backends read a batch in one native
call and hand back the same bytes; everything above that crossing — the readers, the
workbook, `Batch` and every value and verdict it builds — is the same Ruby on either, and
`spec/native_backend_spec.rb` holds the two to the same answer over every cell of both corpora.
Either backend crosses once per batch, yet they don't read at the same speed, and the cause
hasn't been looked into. Through `benchmark/workbook_benchmark.rb`'s eight-column plan on an Apple
M6 on 2026-11-08, making every value, Fiddle is the slower: an xlsx sheet of 300k rows reads in
704 ms against the extension's 458 ms (54% slower), an ods one in 1.02 s against 787 ms (30%), and a
Numbers one in 887 ms against 875 ms. The extension also buys a platform gem with no shared library
for Fiddle to find, and Ruby in the browser, where there is no Fiddle at all.

`HYPERTABULAR_PURE` forces `:fiddle`, as `HYPERCAST_PURE` does for hypercast. It is a testing
and diagnostic switch — CI runs the whole suite through it — read for presence, not value. A
forced backend with nothing to load does not fall through: the first reader raises, and
`HyperTabular.available?` answers `false`. Inside a platform gem, which carries no Fiddle
library, that `LoadError` names the universal gem (`gem install hypertabular --platform ruby`,
or Bundler's `force_ruby_platform`). `HyperTabular.available?` and `HyperTabular.native_version`
answer whether the core resolved, without the first read being what finds out.

**Threads.** A reader or a sheet is one thread's at a time, on either backend. The extension
runs under the GVL; Fiddle releases it for each call.

## Ruby in the browser

ruby.wasm cannot load an extension at runtime: `rbwasm build` links the extension of every gem
in a Gemfile into the one interpreter it builds. So the browser gets a gem of its own,
`hypertabular-wasm` — the same library and the same Magnus extension, prebuilt for
`wasm32-wasip1` — which brings `hypercast-wasm`, HyperCast's own, with it. List it **instead
of** `hypertabular` in the Gemfile you build the interpreter from:

```ruby
source "https://rubygems.org"

gem "hypertabular-wasm"
gem "js" # JavaScript interop, which a browser app almost always wants

group :development do
  gem "ruby_wasm"
end
```

```sh
bundle install
bundle exec rbwasm build --ruby-version 4.0 -o ruby.wasm
```

Load `ruby.wasm` with [`@ruby/wasm-wasi`](https://www.npmjs.com/package/@ruby/wasm-wasi), then
`require "/bundle/setup"` and `require "hypertabular"` as anywhere else. A workbook is read
from its bytes (`HyperTabular::Workbook.new(bytes)`) — a `fetch`ed file, an upload — and
delimited text from a String or any IO.

- **Ruby 3.4 and 4.0**, one archive each, picked by `--ruby-version`; any other minor stops the
  build naming the ones the gem carries. Built and tested against ruby_wasm 2.10.
- **No Rust toolchain.** The gem's `extconf.rb` only hands rbwasm the prebuilt archive. The
  first `rbwasm build` compiles Ruby itself and takes 15–20 minutes; later builds reuse it.
- **macOS with Homebrew: limit pkg-config to `openssl@3`.** Run the build as
  `PKG_CONFIG_LIBDIR="$(brew --prefix openssl@3)/lib/pkgconfig" bundle exec rbwasm build --ruby-version 4.0 -o ruby.wasm`.
  Without it Homebrew's pkg-config files reach into the build twice: its `openssl` is OpenSSL
  4, which the native Ruby rbwasm builds first does not compile against, and its `libyaml`
  is a macOS library that psych then links into the wasm interpreter, which fails for Ruby
  4.0.
- **Static linking only,** into ruby.wasm's default `wasm32-unknown-wasip1` interpreter.
- **Beside HyperCast.** Every Rust extension that carries std defines a few of the same
  symbols (`rust_eh_personality`, rb-sys's `ruby_abi_version`, one of std's); this gem and
  `hypercast-wasm` each rename them to names of their own when CI builds the archive, and the
  build fails if a newer Rust starts exporting another.

What stands behind it: CI builds each minor's archive from the commit, packs the gem the way
it ships, links it into a fresh interpreter and runs [`wasm-smoke/test.rb`](wasm-smoke/test.rb)
under Node and in headless Chrome (the forge's `hyper-build-wasm.yml`). The published gem is
packed around those attested archives.

## Development

```sh
cd rust && cargo cdylib                 # builds rust/target/release/libhypertabular.*
cd ../ruby && bundle install
HYPERTABULAR_PURE=1 bundle exec rspec   # the Fiddle backend: replays both corpora
bundle exec rake native:dev             # builds the Magnus extension for this Ruby, staged
bundle exec rspec                       # the extension, and its agreement with Fiddle
bundle exec rake docs:check
```

## Verifying provenance

Every gem RubyGems.org serves — the universal fallback, each of the seven precompiled platform
gems and `hypertabular-wasm` — verifies from the release tag alone: the release attestation,
signed by this repository's `release.yml` at the tag, names every one of them, with the
release inventory as its predicate.

```sh
gem fetch hypertabular -v <version> --platform <platform>   # or omit --platform for the universal gem
gh attestation verify hypertabular-<version>-<platform>.gem --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Each gem's build provenance is this repository's too: the `ruby-pack` job attests
`ruby/pkg/*.gem` as soon as they are built and `rubygems-publish` pushes those preserved
bytes, so plain `--repo` verifies it:

```sh
gh attestation verify hypertabular-<version>-<platform>.gem --repo SkunkWerkx/HyperTabular
```

That is the release's second layer of checking, not the only one: before any gem is built, the
same job verifies every native binary it packs — the Fiddle libraries, the Magnus extensions
(one per Ruby ABI per platform), the `wasm32-wasip1` archives — against *their own* build
provenance, signed from `SkunkWerkx/.github` by `hyper-build-native.yml` and
`hyper-build-wasm.yml` (so that check needs `--signer-repo SkunkWerkx/.github` added), and
refuses to proceed on an unverified one. RubyGems.org has no unpublish and no
duplicate-version overwrite, so all of this happens while a bad artifact is still reversible,
and the push itself is a separate dispatch from the preserved bytes. The release run then
re-fetches every gem from rubygems.org and fails unless each matches the attested bytes
exactly, turning "rubygems.org stores an upload verbatim" into a per-release check rather than
an assumption. The root README's [Provenance](../README.md#provenance) section has the whole
picture.
