reasons <- c("empty", "malformed", "out_of_range")

# One plan column's columns, as the C shim wrote them, as a HyperCast verdict: the shape
# hypercast's own doors return (hypercast::hypercast_verdict), with `raw` after it.
as_verdict <- function(cols, column) {
  n <- length(cols$ok)
  cols$reason <- structure(cols$reason, levels = reasons, class = "factor")
  switch(column$door,
    timestamp = , unix = , excel_serial = {
      cols$value <- .POSIXct(cols$value, tz = "UTC")
    },
    date = , date_ordered = {
      cols$value <- structure(cols$value, class = "Date")
    },
    datetime = {
      cols$date <- structure(cols$date, class = "Date")
    },
    time = , duration = {
      cols$value <- structure(cols$value, units = "secs", class = "difftime")
    }
  )
  structure(cols,
            class = c("hypercast_verdict", "data.frame"),
            row.names = if (n > 0L) c(NA_integer_, -n) else integer(),
            door = paste0("col_", column$factory))
}

# A batch as the C shim wrote it (unclassed columns), classed for R.
new_batch <- function(raw, plan, names) {
  columns <- lapply(seq_along(plan), function(i) as_verdict(raw$columns[[i]], plan[[i]]))
  names(columns) <- names
  structure(list(columns = columns, line = raw$line), class = "hypertabular_batch")
}

# Batches, unclassed, bound into one, then classed: a whole read.
bind_batches <- function(raws, plan, names) {
  columns <- lapply(seq_along(plan), function(i) {
    parts <- lapply(raws, function(raw) raw$columns[[i]])
    if (!length(parts)) {
      parts <- list(empty_columns(plan[[i]]))
    }
    cols <- lapply(names(parts[[1]]), function(field) {
      unlist(lapply(parts, `[[`, field), use.names = FALSE)
    })
    names(cols) <- names(parts[[1]])
    as_verdict(cols, plan[[i]])
  })
  names(columns) <- names
  line <- unlist(lapply(raws, `[[`, "line"), use.names = FALSE)
  list(columns = columns, line = if (is.null(line)) numeric() else line)
}

# The columns of a verdict of no rows, for a door.
empty_columns <- function(column) {
  value <- switch(column$door,
    bool = logical(),
    i8 = , i16 = , u8 = , u16 = integer(),
    i64 = , u64 = , uuid = , decimal = , text = , datetime = character(),
    numeric()
  )
  extras <- switch(column$door,
    timestamp = , unix = , excel_serial = , duration = list(seconds = numeric(),
                                                            nanos = integer()),
    datetime = list(date = numeric(), nanos_of_day = numeric()),
    time = list(nanos = numeric()),
    list()
  )
  c(list(value = value), extras,
    list(ok = logical(), reason = integer(), offset = integer(), length = integer(),
         raw = character()))
}

#' Batches and tables
#'
#' A read delivers its rows as a `hypertabular_batch`: one HyperCast verdict per plan column,
#' and the line each row came from. `read_batch()` returns one batch at a time;
#' [read_delimited()] and [read_sheet()] return the whole read as a `hypertabular_table`, a
#' batch that also carries the `header` and the `failure` the read ended on.
#'
#' `columns` is a named list holding, per plan column, a `hypercast_verdict` (see
#' [hypercast::hypercast_verdict]): a data frame of one row per row read, with the value
#' where the cell cast (in the R type HyperCast's own door returns for it, and its exact parts
#' where one column cannot carry them), and `ok`, `reason`, `offset` and `length` where it
#' did not. Every cell is a verdict; no cell is an error. `hypercast::is_ok()`,
#' `hypercast::faults()` and `hypercast::optional()` read these verdicts as they read
#' hypercast's own.
#'
#' Each verdict also has a `raw` column: the text of each faulted cell, the text its
#' `offset` and `length` index (`NA` where the cell cast). For delimited text that is the
#' cell as the file holds it, quotes resolved; for a workbook, a text cell's text, or a typed
#' cell said as text (the whole of which its span covers). The span counts in the units
#' `substr()` counts `raw` in, so `substr(raw, offset + 1, offset + length)` is the offending
#' text: characters, or bytes for a cell that is not valid UTF-8, whose `raw` is marked
#' `"bytes"`. A cell holding a NUL byte, which no R string can, has `raw` `NA` and a span in
#' bytes, and through `col_text()` is `out_of_range` at the NUL.
#'
#' `line` is, per row, the 1-based line its record starts on (delimited text) or its 1-based
#' row number in the sheet.
#'
#' `as.data.frame()` gives the values alone, one column per plan column, `NA` where a cell
#' did not cast.
#'
#' @param x A `hypertabular_batch`.
#' @param row.names,optional,... As for [as.data.frame()]; `...` is passed on to `print()`.
#' @return `as.data.frame()` a data frame; `print()` returns `x` invisibly.
#' @name hypertabular_batch
#' @family reads
#' @examples
#' csv <- charToRaw("id,amount\n1,2.5\n2,x\n3,\n")
#' t <- read_delimited(csv, list(id = col_i32("id"), amount = col_f64("amount")))
#' t
#' t$columns$amount
#' as.data.frame(t)
#' hypercast::faults(t$columns$amount)
NULL

#' @rdname hypertabular_batch
#' @export
as.data.frame.hypertabular_batch <- function(x,
                                             row.names = NULL, # nolint: object_name_linter.
                                             optional = FALSE, ...) {
  values <- lapply(x$columns, function(v) v$value)
  as.data.frame(structure(values, class = "data.frame",
                          row.names = .set_row_names(length(x$line))),
                row.names = row.names, optional = optional)
}

#' @rdname hypertabular_batch
#' @export
print.hypertabular_batch <- function(x, ...) {
  rows <- length(x$line)
  cat(sprintf("<%s> %d row%s, %d column%s\n", class(x)[[1]], rows, if (rows == 1L) "" else "s",
              length(x$columns), if (length(x$columns) == 1L) "" else "s"))
  for (name in names(x$columns)) {
    ok <- is_ok(x$columns[[name]])
    cat(sprintf("  %s (%s): %d ok, %d faulted\n", name, attr(x$columns[[name]], "door"),
                sum(ok %in% TRUE), sum(ok %in% FALSE)))
  }
  if (!is.null(x$failure)) {
    cat("  ended on a failure: ", format(x$failure), "\n", sep = "")
  }
  invisible(x)
}
