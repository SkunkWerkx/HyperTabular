# HyperTabular for .NET

Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
typed columns, with a [HyperCast](https://github.com/SkunkWerkx/HyperCast) `Verdict` for
every cell.

```csharp
using HyperCast;
using HyperTabular;

Column[] plan = [Column.Int32(0), Column.Text(1), Column.Double(2)];
using var reader = DelimitedReader.Open("orders.csv", Dialect.Csv, plan);

while (reader.Read())
{
    // A column at a time, as the core wrote it…
    ReadOnlySpan<int> ids = reader.Values<int>(0);
    ReadOnlySpan<CellVerdict> verdicts = reader.Verdicts(0);

    // …or a cell at a time, as HyperCast's union.
    for (var row = 0; row < reader.Rows; row++)
    {
        var line = reader.Double(2, row) switch
        {
            Success<double> score => $"{reader.GetString(1, row)}: {score.Value}",
            Fault fault => $"row {row}: {fault.Reason} in \"{Encoding.UTF8.GetString(reader.Raw(2, row))}\"",
        };
    }
}
```

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
allocates the buffers — the input, one value array and one verdict array per column, the
table that locates each cell — once, pinned, and reuses them for every batch. The core
fills them in one native call per batch: the boundary is crossed once per few thousand rows,
not once per cell.

- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  the plan states each column's door and, for numbers, its `NumFormat`.
- **HyperCast is the judge.** `Verdict<T>`, `Fault`, `NumFormat`, `UnixPrecision`,
  `DateOrder` and `ExcelEpoch` are HyperCast's own types, from its package. A cell means
  exactly what `Cast` would say of the same text.
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on. A record of the wrong width, or input
  that ends inside a quoted cell, is a `TabularException`, raised after every intact row
  before it.
- **Text is zero-copy.** `TryGetText` hands back a span into the reader's own buffer; only
  a cell with `""` inside is unescaped, into an arena.
- **Native AOT.** Source-generated `LibraryImport` only. An AOT publish links the core in
  from the package's static archive: one executable, nothing loaded at start.

`Tabular.IsAvailable` and `Tabular.NativeVersion` answer whether the native library
resolved, without the first read being what finds out.
