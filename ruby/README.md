# hypertabular

Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
typed columns, with a [HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for
every cell.

```ruby
require "hypertabular"

plan = [
  HyperTabular::Column.i32(0),
  HyperTabular::Column.text(1),
  HyperTabular::Column.f64(2)
]

HyperTabular::DelimitedReader.open("orders.csv", HyperTabular::Dialect::CSV, plan) do |reader|
  reader.header                       # => ["id", "name", "score"]

  while reader.read
    # A column at a time, decoded in one pass — nil where a cell did not cast…
    ids = reader.values(0)

    # …or a cell at a time, as HyperCast's union.
    reader.rows.times do |row|
      case reader.verdict(2, row)
      in HyperCast::Success(value:)
        puts "#{reader.values(1)[row]}: #{value}"
      in HyperCast::Fault(reason:, offset:, length:)
        warn "row #{row}: #{reason} in #{reader.raw(2, row).inspect}"
      end
    end
  end
end
```

`reader.each_row` walks every remaining row as an Array of verdicts, for when a row at a
time is what the caller wants.

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
allocates the buffers — one value array and one verdict array per column, the table that
locates each cell — once, and reuses them for every batch. The core fills them in one
native call per batch, and a column comes out of its buffer in one `String#unpack`: the
boundary is crossed once per few thousand rows, not once per cell.

- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  the plan states each column's door and, for numbers, its `HyperCast::NumFormat`.
- **HyperCast is the judge.** `HyperCast::Success`, `HyperCast::Fault`,
  `HyperCast::NumFormat`, `HyperCast::Decimal` and the declared options (`:milliseconds`,
  `:day_month_year`, `:y1904`) are the `hypercast` gem's own. A cell means exactly what
  HyperCast's door would say of the same text, and comes back as the Ruby type that gem
  returns for it.
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on. A record of the wrong width, or input
  that ends inside a quoted cell, is a `HyperTabular::TabularError` carrying `kind`,
  `record`, `line`, `byte`, `expected` and `found`, raised after every intact row before
  it.
- **The text of any cell is to hand.** `reader.raw(column, row)` is what the cell was cast
  from, whatever its door and verdict, and a `Fault`'s span indexes it:
  `raw[fault.offset, fault.length]` is the offending text.
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

## The backend

`HyperTabular::BACKEND` is `:fiddle`: the native library, bundled per platform under
`lib/hypertabular/native/{rid}/`, called through Ruby's own Fiddle — nothing compiles on
install. `HYPERTABULAR_PURE=1` forces that backend, as `HYPERCAST_PURE` does for hypercast;
it is the only one today. `HyperTabular.available?` and `HyperTabular.native_version`
answer whether the native library resolved, without the first read being what finds out.

## Development

```sh
cd rust && cargo cdylib                 # builds rust/target/release/libhypertabular.*
cd ../ruby && bundle install
HYPERTABULAR_PURE=1 bundle exec rspec   # replays corpus/delimited.json, among the rest
bundle exec rake docs:check
```
