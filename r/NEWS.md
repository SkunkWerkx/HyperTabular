# hypertabular 1.1.0

## Added

* First release of the R binding to the HyperTabular core, built from source with the
  vendored core crate and the hypercast crate it casts with (no network access at install).
* Delimited text: `delimited_dialect()`, `delimited_reader()` over a raw vector, a file path
  or a connection, and `read_delimited()`.
* Workbooks: `workbook()` opens an XLSX, ODS or Numbers file and lists its sheets;
  `sheet_reader()` and `read_sheet()` read one.
* Plans of columns, one factory per HyperCast door: `col_bool()`, `col_i8()`, `col_i16()`,
  `col_i32()`, `col_i64()`, `col_u8()`, `col_u16()`, `col_u32()`, `col_u64()`, `col_f32()`,
  `col_f64()`, `col_decimal()`, `col_uuid()`, `col_timestamp()`, `col_unix()`,
  `col_excel_serial()`, `col_date()`, `col_datetime()`, `col_time()`, `col_duration()` and
  `col_text()`, naming their source by header name or position.
* `header()`, `bind()` and `read_batch()`: a batch at a time, with every cell a
  `hypercast_verdict` row and the faulted cell's text beside its span.
* Structural failures are `hypertabular_failure` values, held by `failure()`, never R errors.
* `hypertabular_native_version()`: the version of the loaded Rust core.
