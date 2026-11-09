# hypertabular 1.1.0

## Added

* First release of the R binding to the HyperTabular core, built from source with the
  vendored core crate, the hypercast crate it casts with and simdutf8, which hypercast checks
  UTF-8 with (no network access at install).
* Delimited text: `delimited_dialect()`, `delimited_reader()` over a raw vector, a file path
  or a connection, and `read_delimited()`.
* Workbooks: `workbook()` opens an XLSX, ODS or Numbers file and lists its sheets;
  `sheet_reader()` and `read_sheet()` read one.
* Dialects and readers declare `skip`, `header_row`, `fill`, `long_rows`, `na` and `encoding`
  (`"utf-8"`, `"windows-1252"`, `"latin-1"`, `"utf-16le"`); sheets declare `skip`, `header_row`,
  `na` and a `range`.
* Plans of columns, one factory per HyperCast door: `col_bool()`, `col_i8()`, `col_i16()`,
  `col_i32()`, `col_i64()`, `col_u8()`, `col_u16()`, `col_u32()`, `col_u64()`, `col_f32()`,
  `col_f64()`, `col_decimal()`, `col_uuid()`, `col_timestamp()`, `col_unix()`,
  `col_excel_serial()`, `col_date()`, `col_datetime()`, `col_time()`, `col_duration()` and
  `col_text()`, naming their source by header name or position, each with its own `na`;
  `col_timestamp()`, `col_date()` and `col_datetime()` take `options`, the last two `months`, and
  `col_text(raw = TRUE)` gives each cell's bytes. A format the core refuses is an R error naming
  the column, before the source opens; a name the header holds twice is refused.
* `header()`, `bind()` and `read_batch()`: a batch at a time, with every cell a
  `hypercast_verdict` row with its `detail` and `origin`, and the faulted cell's text beside its
  span. Text R can't hold is `ok` with an `NA` value: checking `ok` alone doesn't guarantee a usable
  character value.
* `problems()` lists a batch's or a table's problems when asked, a read paying nothing for it.
* `suggest_plan()` samples a source and prints a plan to paste; its `$plan` reads back.
* `close()` on readers and workbooks, safe twice. A call into a reader from inside its own read
  is an R error. An interrupt mid-read loses nothing inside the read: the reader reads on, every
  row once and numbered right, or refuses every further read, naming the interrupt; `close()`
  always works. An interrupt landing in the caller's own code after `read_batch()` returns can
  lose that one batch, which no R function can prevent; it shows as a gap in `row`.
* Structural failures are `hypertabular_failure` values, held by `failure()`, never R errors.
* `hypertabular_native_version()`: the version of the loaded Rust core.
