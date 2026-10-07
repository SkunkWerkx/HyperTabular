# hypertabular

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```ruby
require "hypertabular"

plan = [
  HyperTabular::Column.i32(0),
  HyperTabular::Column.text(1),
  HyperTabular::Column.f64(2)
]

HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV, plan) do |reader|
  reader.header                       # => ["id", "name", "score"]

  while (batch = reader.read)
    # A column at a time, decoded in one pass — nil where a cell did not cast…
    ids = batch.values(0)

    # …or a cell at a time, as HyperCast's union.
    batch.rows.times do |row|
      case batch.get(2, row)
      in HyperCast::Success(value:)
        puts "#{batch.values(1)[row]}: #{value}"
      in HyperCast::Fault(reason:, offset:, length:)
        warn "line #{batch.line(row)}: #{reason} in #{batch.raw(2, row).inspect}"
      end
    end
  end
end

# A workbook reads into the same batch.
book = HyperTabular::Workbook.open("orders.xlsx")
book.sheets                           # => [#<data HyperTabular::SheetInfo name="Orders", hidden=false>]
sheet = book.sheet("Orders", HyperTabular::SheetOptions::DEFAULT, plan)
while (batch = sheet.read)
  # …
end
```

`reader.each_row` walks every remaining row as an Array of verdicts, for when a row at a
time is what the caller wants; `sheet.each_batch` walks a sheet's batches.

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — one value array and one verdict array per column, the
table that locates each cell — once, and reuse them for every batch. The core fills them in one
native call per batch, and a column comes out of its buffer in one `String#unpack`: the
boundary is crossed once per few thousand rows, not once per cell.

- **One batch class.** `read` returns a `HyperTabular::Batch` — `rows`, `columns`,
  `line(row)`, `values(column)`, `verdicts(column)`, `get(column, row)` and `raw(column,
  row)` — or `nil` once there are no more rows, for delimited text and a sheet alike. A
  batch owns what it shows: it stays good after the reader has moved on.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header and whether empty rows are skipped; the plan
  states each column's door and, for numbers, its `HyperCast::NumFormat`.
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
- **A String is read in place.** An IO is read forward only, through a buffer that grows
  when a record does not fit it.

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
| `:fiddle` | `libhypertabular` for this platform, `dlopen`ed through Fiddle | no extension loads: the universal gem, on a Ruby or platform no platform gem covers (Intel macOS, Ruby 3.3) |

Selection happens once, at `require`, in that order. Both backends read a batch in one native
call and hand back the same bytes; everything above that crossing — the readers, the
workbook, `Batch` and every value and verdict it builds — is the same Ruby on either, and
`spec/native_backend_spec.rb` holds the two to the same answer over every cell of both corpora.
So the extension is not where a read gets faster (the crossing was already once per batch);
what it buys is a platform gem with no shared library for Fiddle to find, and Ruby in the
browser, where there is no Fiddle at all.

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
