failure_kinds <- c(
  "1" = "unclosed_quote", "2" = "column_count", "3" = "row_too_long", "16" = "not_a_zip",
  "17" = "container", "18" = "encrypted", "19" = "method", "20" = "missing_part", "21" = "xml",
  "22" = "deflate", "23" = "not_a_workbook", "24" = "shared_string", "25" = "too_large",
  "26" = "iwa"
)

# The C shim's failure (code, line, record, byte, expected, found) as R gets it.
new_failure <- function(v) {
  kind <- failure_kinds[as.character(v[[1]])]
  structure(
    list(kind = if (is.na(kind)) "container" else unname(kind), record = v[[3]], line = v[[2]],
         byte = v[[4]], expected = v[[5]], found = v[[6]]),
    class = "hypertabular_failure"
  )
}

#' Structural failures
#'
#' A file that is broken (a record with the wrong number of cells, an input that ends inside
#' a quoted cell, a workbook that is not a zip, is encrypted, or has a part missing or cut
#' short) is a `hypertabular_failure`: a value, never an R error. It comes after every intact
#' row before the break has been delivered, and is final, since a forward-only read has no
#' point to resume from: a reader's `failure()` holds it from then on, and every later
#' `read_batch()` returns `NULL`. [read_delimited()] and [read_sheet()] return the rows before
#' it with it, as their `failure`; [workbook()] returns a workbook that could not be opened
#' with its `failure` set. A cell that does not cast is never a failure: it is a verdict.
#'
#' `kind` is one of `"unclosed_quote"`, `"column_count"`, `"row_too_long"` (delimited
#' text), `"not_a_zip"`, `"container"`, `"encrypted"`, `"method"`, `"missing_part"`,
#' `"xml"`, `"deflate"`, `"not_a_workbook"`, `"shared_string"`, `"too_large"` and `"iwa"`
#' (workbooks). For delimited text `record` is the zero-based index of the offending record
#' (the header and skipped blank lines counted), `line` the 1-based line it starts on and
#' `byte` its byte offset; for `"column_count"`, `expected` is the cells of the first record
#' and `found` this one's. For a workbook `record` is the part being read (`6` a worksheet,
#' `10` a Numbers part), `line` the sheet row and `byte` the offset in the part's inflated
#' bytes.
#'
#' @param x A reader (from [delimited_reader()] or [sheet_reader()]), or a
#'   `hypertabular_failure`.
#' @param ... Ignored.
#' @return `failure()` returns the reader's `hypertabular_failure`, or `NULL` while it has
#'   none. `format()` returns a sentence describing the failure.
#' @name hypertabular_failure
#' @family reads
#' @examples
#' t <- read_delimited(charToRaw("a,b\n1,2\n3\n"), list(col_i32("a")))
#' t$failure
#' t$columns[[1]]$value
NULL

#' @rdname hypertabular_failure
#' @export
failure <- function(x) {
  check_reader(x)
  x$failure
}

#' @rdname hypertabular_failure
#' @export
format.hypertabular_failure <- function(x, ...) {
  at <- sprintf("%s (line %s, byte %s)", fmt(x$record), fmt(x$line), fmt(x$byte))
  switch(x$kind,
    column_count = sprintf("Record %s has %s cells; the first record had %s.", at,
                           fmt(x$found), fmt(x$expected)),
    unclosed_quote = sprintf("The input ended inside a quoted cell in record %s.", at),
    row_too_long = sprintf("Record %s exceeds the 1073741824-byte row ceiling.", at),
    not_a_zip = "The workbook is not a zip file.",
    encrypted = "The workbook is encrypted.",
    method = sprintf(paste("Part %s of the workbook is compressed by method %s, neither",
                           "stored nor deflate."), fmt(x$record), fmt(x$found)),
    missing_part = sprintf("Part %s, which the workbook cannot be read without, is missing.",
                           fmt(x$record)),
    xml = sprintf("Part %s of the workbook ends inside an XML construct (byte %s).",
                  fmt(x$record), fmt(x$byte)),
    deflate = sprintf("Part %s of the workbook is not a whole deflate stream (byte %s).",
                      fmt(x$record), fmt(x$byte)),
    not_a_workbook = "The zip is not an XLSX, ODS or Numbers workbook.",
    shared_string = sprintf("Row %s names shared string %s; the table has %s.", fmt(x$line),
                            fmt(x$found), fmt(x$expected)),
    too_large = "The workbook holds more text than a batch can address.",
    iwa = sprintf(paste("Part %s of the Numbers document is not what the IWA packaging says",
                        "(byte %s, detail %s)."), fmt(x$record), fmt(x$byte), fmt(x$found)),
    "The workbook's zip structure is broken."
  )
}

fmt <- function(n) format(n, scientific = FALSE, trim = TRUE)

#' @rdname hypertabular_failure
#' @export
print.hypertabular_failure <- function(x, ...) {
  cat(sprintf("<hypertabular_failure: %s> %s\n", x$kind, format(x)))
  invisible(x)
}
