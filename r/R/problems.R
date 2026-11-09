problem_kinds <- c("cell", "row", "host_conversion")

#' What went wrong in a read
#'
#' `problems()` lists what went wrong in a batch or a table, one row per problem, computed when
#' it is called from the verdict columns the batch already holds, so a read pays nothing for
#' it. It lists every cell that is `malformed` or `out_of_range`, every workbook formula with
#' no cached value, every record `long_rows = "report"` cut to the width, and every cell the
#' core accepted whose text R can't hold. It leaves out blanks, cells a declared `na` matched,
#' and cells a short record lacked under `fill = TRUE`: those are declared, not wrong.
#'
#' The columns are `row` (the row's place among the rows the read delivered, from 1), `line`
#' (the record's first line, or the sheet's row number), `sheet` (the sheet's name; `NA` for
#' delimited text), `column` (the output column's name; `NA` for a row), `source_column` (the
#' column in the source, from 1: a delimited plan column's position, a sheet column's own, `A`
#' being 1, wherever a range starts; `NA` for a row), `kind` (`"cell"`, `"row"` for a record cut
#' to the width, or `"host_conversion"` for a cell `ok` with an `NA` value), `reason`, `detail`
#' and `origin` (the verdict's; `NA` for a row), `offset`, `length` and `raw` (the verdict's
#' span and text; `NA` for a row and a host conversion, which has no span), and `expected` and
#' `found` (a cut record's width and its own; `NA` otherwise).
#'
#' @param x A `hypertabular_batch` or `hypertabular_table`.
#' @return A data frame, one row per problem, in the order the rows were read.
#' @family reads
#' @export
#' @examples
#' t <- read_delimited(charToRaw("title\na,b\n1,x\n2,3,4\n"), skip = 1, long_rows = "report",
#'                     plan = list(a = col_i32("a"), b = col_i32("b")))
#' problems(t)
problems <- function(x) {
  if (!inherits(x, "hypertabular_batch")) {
    stop("x must be a batch or a table, as read_batch(), read_delimited() or read_sheet() returns",
         call. = FALSE)
  }
  rows <- length(x$line)
  context <- x$context
  names <- names(x$columns)
  codes <- lapply(x$columns, function(v) {
    origin <- as.integer(v$origin) - 1L
    origin[!is.na(origin) & origin >= length(origins)] <- 0L
    list(as.integer(v$reason), as.integer(v$detail_code), origin)
  })
  listed <- .Call(C_faults, unname(codes), rows,
                  if (!is.null(x$widths) && length(x$widths) == rows) as.numeric(x$widths),
                  context$expected %||% 0, context$long_rows %||% 0L)
  row <- listed[[1]] + 1
  plan_column <- listed[[2]] + 1
  kind <- ifelse(plan_column == 0, 2L, 1L)
  # A text cell the core accepted whose text R can't hold: ok, with an NA value.
  host <- lapply(seq_along(x$columns), function(j) {
    v <- x$columns[[j]]
    if (!identical(context$doors[[j]], "text") || isTRUE(context$raws[[j]])) {
      return(integer())
    }
    which(v$ok %in% TRUE & is.na(v$value))
  })
  host_rows <- unlist(host)
  host_columns <- rep(seq_along(host), lengths(host))
  row <- c(row, host_rows)
  plan_column <- c(plan_column, host_columns)
  kind <- c(kind, rep(3L, length(host_rows)))
  expected <- c(ifelse(kind[seq_along(listed[[1]])] == 2L, listed[[6]], NA),
                rep(NA, length(host_rows)))
  found <- c(ifelse(kind[seq_along(listed[[1]])] == 2L, listed[[7]], NA),
             rep(NA, length(host_rows)))
  # In the order the rows were read: within a row the row's own entry, then its cells.
  by <- order(row, plan_column)
  row <- row[by]
  plan_column <- plan_column[by]
  kind <- kind[by]
  expected <- as.numeric(expected[by])
  found <- as.numeric(found[by])
  cell <- kind != 2L
  pick <- function(field, missing) {
    out <- rep(missing, length(row))
    for (j in unique(plan_column[cell])) {
      at <- cell & plan_column == j
      values <- x$columns[[j]][[field]]
      if (is.factor(values)) values <- as.integer(values)
      out[at] <- values[row[at]]
    }
    out
  }
  spanned <- kind == 1L
  offset <- pick("offset", NA_integer_)
  length <- pick("length", NA_integer_)
  raw <- pick("raw", NA_character_)
  offset[!spanned] <- NA_integer_
  length[!spanned] <- NA_integer_
  raw[!spanned] <- NA_character_
  ordinal <- context$ordinals[plan_column] %||% rep(NA_real_, length(row))
  out <- data.frame(
    row = x$row[row] %||% row,
    line = x$line[row],
    sheet = rep(context$sheet %||% NA_character_, length(row)),
    column = as.character(ifelse(cell, names[plan_column], NA_character_)),
    source_column = as.numeric(ifelse(cell, ordinal + (context$first_column %||% 1), NA_real_)),
    kind = structure(as.integer(kind), levels = problem_kinds, class = "factor"),
    reason = structure(as.integer(ifelse(cell, pick("reason", NA_integer_), NA_integer_)),
                       levels = reasons, class = "factor"),
    detail = detail_factor(as.integer(ifelse(cell, pick("detail_code", NA_integer_),
                                             NA_integer_))),
    origin = origin_factor(as.integer(ifelse(cell, pick("origin", NA_integer_) - 1L,
                                             NA_integer_))),
    offset = offset,
    length = length,
    raw = raw,
    expected = expected,
    found = found,
    stringsAsFactors = FALSE
  )
  rownames(out) <- NULL
  out
}
