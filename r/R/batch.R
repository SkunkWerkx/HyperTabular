reasons <- c("empty", "malformed", "out_of_range")
origins <- c("cell", "token", "padded", "uncached")
detail_levels <- function() c(names(hypercast::fault_details), "unknown")

# The core's detail codes as the factor a verdict carries, as hypercast builds its own: a code
# the installed hypercast names as its name, one it doesn't as "unknown", and none as NA.
detail_factor <- function(code) {
  levels <- detail_levels()
  index <- match(code, hypercast::fault_details)
  index[!is.na(code) & is.na(index)] <- length(levels)
  structure(index, levels = levels, class = "factor")
}

# The core's origin codes as a factor: a code this version doesn't name is "unknown".
origin_factor <- function(code) {
  levels <- c(origins, "unknown")
  index <- code + 1L
  index[!is.na(index) & (index < 1L | index > length(origins))] <- length(levels)
  structure(index, levels = levels, class = "factor")
}

# One plan column's columns, as the C shim wrote them, as a HyperCast verdict: the shape
# hypercast's own doors return (hypercast::hypercast_verdict), then `raw` and `origin`.
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
  raw <- cols$raw
  origin <- cols$origin
  code <- cols$detail
  cols$raw <- NULL
  cols$origin <- NULL
  cols$detail <- detail_factor(code)
  cols$detail_code <- code
  cols$raw <- raw
  cols$origin <- origin_factor(origin)
  structure(cols,
            class = c("hypercast_verdict", "data.frame"),
            row.names = .set_row_names(n),
            door = paste0("col_", column$factory))
}

# What a batch keeps for problems(): where each plan column reads from and what the read was.
read_context <- function(reader) {
  expected <- if (inherits(reader, "hypertabular_delimited_reader") && !is.null(reader$ptr) &&
                    !isTRUE(reader$closed)) {
    .Call(C_delimited_width, reader$ptr)
  } else {
    0
  }
  list(sheet = reader$sheet, ordinals = reader$ordinals, first_column = reader$first_column,
       expected = expected, long_rows = reader$long_rows,
       raws = vapply(reader$plan, function(column) isTRUE(column$raw), FALSE),
       doors = vapply(reader$plan, function(column) column$door, ""))
}

# A batch as the C shim wrote it (unclassed columns), classed for R.
new_batch <- function(raw, reader) {
  plan <- reader$plan
  columns <- lapply(seq_along(plan), function(i) as_verdict(raw$columns[[i]], plan[[i]]))
  names(columns) <- repair_names(reader$names)
  structure(list(columns = columns, line = raw$line, row = raw$first + seq_along(raw$line),
                 widths = raw$widths, original_names = reader$names,
                 context = read_context(reader)),
            class = "hypertabular_batch")
}

# Batches, unclassed, bound into one, then classed: a whole read.
bind_batches <- function(raws, plan, names) {
  columns <- lapply(seq_along(plan), function(i) {
    parts <- lapply(raws, function(raw) raw$columns[[i]])
    if (!length(parts)) {
      parts <- list(empty_columns(plan[[i]]))
    }
    cols <- lapply(names(parts[[1]]), function(field) {
      if (is.list(parts[[1]][[field]])) {
        do.call(c, lapply(parts, `[[`, field))
      } else {
        unlist(lapply(parts, `[[`, field), use.names = FALSE)
      }
    })
    names(cols) <- names(parts[[1]])
    as_verdict(cols, plan[[i]])
  })
  names(columns) <- repair_names(names)
  line <- unlist(lapply(raws, `[[`, "line"), use.names = FALSE)
  widths <- unlist(lapply(raws, `[[`, "widths"), use.names = FALSE)
  list(columns = columns, line = line %||% numeric(), widths = widths)
}

# The columns of a verdict of no rows, for a door, as the C shim makes them.
empty_columns <- function(column) {
  value <- if (isTRUE(column$raw)) list() else switch(column$door,
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
         raw = character(), detail = integer(), origin = integer()))
}

new_table <- function(reader, raws, plan, names) {
  bound <- bind_batches(raws, plan, names)
  structure(
    list(header = reader$header, columns = bound$columns, line = bound$line,
         row = seq_along(bound$line), widths = bound$widths, original_names = names,
         context = if (is.environment(reader)) read_context(reader) else NULL,
         failure = reader$failure),
    class = c("hypertabular_table", "hypertabular_batch")
  )
}

#' Batches and tables
#'
#' A read delivers its rows as a `hypertabular_batch`: one HyperCast verdict per plan column,
#' and where each row came from. `read_batch()` returns one batch at a time;
#' [read_delimited()] and [read_sheet()] return the whole read as a `hypertabular_table`, a
#' batch that also carries the `header` and the `failure` the read ended on. Every vector in
#' either is R's own, and stays good after the reader moves on, is closed, or is collected.
#'
#' `columns` is a named list holding, per plan column, a `hypercast_verdict` (see
#' [hypercast::hypercast_verdict]): a data frame of one row per row read, with the value
#' where the cell cast (in the R type HyperCast's own door returns for it, and its exact parts
#' where one column cannot carry them), and `ok`, `reason`, `offset`, `length`, `detail` and
#' `detail_code` where it did not. Every cell is a verdict; no cell is an error.
#' `hypercast::is_ok()`, `hypercast::faults()` and `hypercast::optional()` read these verdicts
#' as they read hypercast's own. A detail code the installed hypercast has no name for reads
#' as `"unknown"`, its number kept in `detail_code`. The names of `columns` are the plan's
#' (else the header's, else `V` and the position), made unique where they repeat as
#' `make.unique(sep = "_")` makes them; `original_names` keeps them as given.
#'
#' Each verdict also has a `raw` column, the text of each faulted cell, the text its `offset`
#' and `length` index (`NA` where the cell cast), and an `origin` column: `"cell"` for a value
#' the source holds, `"token"` for a cell the read's or the column's `na` matched, `"padded"`
#' for a cell a short record lacked under `fill = TRUE`, and `"uncached"` for a workbook
#' formula with no cached value. For delimited text `raw` is the cell as the file holds it,
#' quotes resolved; for a workbook, a text cell's text, or a typed cell said as text (the whole
#' of which its span covers). The span counts in the units `substr()` counts `raw` in, so
#' `substr(raw, offset + 1, offset + length)` is the offending text: characters, or bytes for a
#' cell that is not valid UTF-8, whose `raw` is marked `"bytes"`.
#'
#' Text R can't hold (a NUL byte inside it, or more than `INT_MAX` bytes) is `ok` with an `NA`
#' value, through `col_text()` as through `hypercast::cast_text()`: `ok` says the core accepted
#' the cell, and the `NA` that R can't represent it. So checking `ok` alone doesn't guarantee a
#' usable character value; `col_text(raw = TRUE)` gives the exact bytes, and [problems()]
#' lists each such cell as a `"host_conversion"` row. A faulted cell holding a NUL has `raw`
#' `NA` and a span in bytes.
#'
#' `line` is, per row, the 1-based line its record starts on (delimited text) or its 1-based
#' row number in the sheet, and `row` its 1-based place among the rows the read delivered.
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
