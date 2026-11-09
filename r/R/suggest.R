# A sample's delimited rows stop here; a sheet's at its own batch size.
sample_cap <- 4096
# A span holds a u32 offset: a sample that far along stops, and says it did.
sample_bytes <- 4294967295
statuses <- c("resolved", "ambiguous", "unresolved")

#' Suggest a plan
#'
#' `suggest_plan()` reads a sample of a source's rows as text and asks the core which door
#' reads each column: bool, `i32`, `i64`, `f64`, an ISO date, a timestamp, a UUID, in that
#' order of preference, or text. A suggestion is never applied: it is a plan to look at, change
#' and pass to a read. Printed, it is that plan as code to paste, each column's status beside
#' it.
#'
#' A column is `"resolved"` when one door and one declaration take every cell of the sample
#' that isn't blank, and the plan reads it through them; `"ambiguous"` when every cell is read
#' but the sample can't say which declaration it is written in (`01/02/2026` under two date
#' orders, `1,234` under two notations), and `"unresolved"` when there is nothing to go on, or
#' a cell no door took beside cells one did. Both of those read as text, which refuses nothing.
#' Declaring what the sample can't say (`format`, `date_order`) and asking again settles it.
#' A resolved number is planned in the format declared for it, or the one notation the sample
#' left, or the invariant one; each is Western, so a later `1,23,456` is refused.
#'
#' The sample is `sample_rows` rows (0 is taken as 1), at most 4096 of delimited text and at
#' most a sheet's `batch_rows`, read in as many batches as the core says fit. `truncated` says
#' whether the source has rows past it: one with exactly the sample's rows hasn't. A failure
#' among the sampled rows is the suggestion's `failure`, with no plan; a structural failure or
#' a connection's own error just past a whole sample only says there is more. Any other
#' condition propagates.
#'
#' @param source Delimited text (a raw vector, a file path, or a connection when the dialect
#'   declares a header: a source with none is opened twice, once for its width), or a
#'   [workbook()].
#' @param dialect A [delimited_dialect()], for delimited text.
#' @param sample_rows The most rows the sample holds.
#' @param format A [hypercast::num_format()] every column's numbers are read in, settling what
#'   `,` and `.` mean in them; `NULL` declares none.
#' @param date_order The order every column's dates are read in where the sample leaves it
#'   open: `"year_month_day"`, `"month_day_year"` or `"day_month_year"`, or `NULL`.
#' @param column_formats,column_date_orders Per source column, ahead of `format` and
#'   `date_order`: a list named by the source columns, each name a header name or a position
#'   from 1. Every format given is judged first of anything, whether or not the sample
#'   reaches its column.
#' @param sheet For a workbook, the sheet, as for [sheet_reader()].
#' @param ... For delimited text, [delimited_reader()]'s `buffer_bytes`, `skip`, `header_row`,
#'   `fill`, `long_rows`, `na` and `encoding`; for a workbook, [sheet_reader()]'s
#'   `has_header`, `skip_empty_rows`, `batch_rows`, `skip`, `header_row`, `na` and `range`.
#' @param x A `hypertabular_suggestion`.
#' @return A `hypertabular_suggestion`: a list of `plan` (a list of columns, one per source
#'   column, named for them, or `NULL` when the sample failed), `report` (a data frame of each
#'   column's `column` position, `name`, `door`, `param` (the door's parameter: a date's order
#'   as 1 `year_month_day`, 2 `month_day_year` or 3 `day_month_year`, the first candidate's where
#'   ambiguous; a Unix precision or an Excel epoch; otherwise 0), `status`,
#'   `matched` and `sampled` counts, `candidates` to choose between where ambiguous, and the
#'   `format` a resolved number is planned in), `truncated`, `rows` (the rows sampled) and
#'   `failure`.
#' @family plans
#' @export
#' @examples
#' csv <- charToRaw("id,when,amount,note\n1,2026-01-07,\"1,234.5\",a\n2,2026-01-08,12,b\n")
#' s <- suggest_plan(csv)
#' s
#' read_delimited(csv, s$plan)$columns$when
#'
#' suggest_plan(charToRaw("d\n01/02/2026\n"))$report[, c("door", "status", "candidates")]
#' suggest_plan(charToRaw("d\n01/02/2026\n"), date_order = "day_month_year")
suggest_plan <- function(source, dialect = delimited_dialect(), sample_rows = 1000,
                         format = NULL, date_order = NULL, column_formats = list(),
                         column_date_orders = list(), sheet = 1L, ...) {
  sample_rows <- max(count_arg(sample_rows, "sample_rows"), 1)
  judge_format(format, "format")
  check_keyed(column_formats, "column_formats")
  for (i in seq_along(column_formats)) {
    judge_format(column_formats[[i]],
                 sprintf("column_formats[[\"%s\"]]", names(column_formats)[[i]]))
  }
  if (!is.null(date_order)) declared(date_order, orders, "date_order")
  check_keyed(column_date_orders, "column_date_orders")
  for (order in column_date_orders) declared(order, orders, "a column's date order")
  asked <- list(format = format, date_order = date_order, column_formats = column_formats,
                column_date_orders = column_date_orders)
  if (inherits(source, "hypertabular_workbook")) {
    return(suggest_sheet(source, sheet, sample_rows, asked, ...))
  }
  suggest_delimited(source, dialect, sample_rows, asked, ...)
}

check_keyed <- function(entries, what) {
  if (!is.list(entries) || (length(entries) &&
                              (is.null(names(entries)) || anyNA(names(entries)) ||
                                 !all(nzchar(names(entries)))))) {
    stop(sprintf("%s must be a list named by source columns", what), call. = FALSE)
  }
}

# A format the core would refuse, refused before anything is opened, in the caller's words.
judge_format <- function(format, what) {
  if (is.null(format)) {
    return(invisible())
  }
  tryCatch({
    plan <- list(col_i32(1, format))
    judge_plan(plan, pack_specs(plan, 0L))
  }, error = function(e) {
    stop(sub("^(format|plan column 1)", what, conditionMessage(e)), call. = FALSE)
  })
}

# What the caller declared for source column `ordinal` (from 0), named `name` in the header.
declared_for <- function(entries, ordinal, name, otherwise) {
  keys <- names(entries)
  for (i in rev(seq_along(entries))) {
    key <- keys[[i]]
    if ((!is.null(name) && !is.na(name) && identical(key, name)) ||
          identical(key, as.character(ordinal + 1L))) {
      return(entries[[i]])
    }
  }
  otherwise
}

# A source opened for one read inside the sampler, called into once at a time: the batch's
# columns, as the shim wrote them, or NULL.
sample_read <- function(reader, sample) {
  x <- reader
  enter(x, "a sample")
  raw <- read_raw_batch(x, sample)
  if (is.null(raw)) {
    return(NULL)
  }
  received(x, raw)
  raw
}

suggest_delimited <- function(source, dialect, sample_rows, asked, ...) {
  dialect <- check_dialect(dialect)
  extra <- list(...)
  unknown <- setdiff(names(extra), c("buffer_bytes", "skip", "header_row", "fill", "long_rows",
                                     "na", "encoding"))
  if (length(unknown) || (length(extra) && is.null(names(extra)))) {
    stop(sprintf("suggest_plan() reads delimited text with no argument %s",
                 if (length(unknown)) unknown[[1]] else "unnamed"), call. = FALSE)
  }
  given <- function(name, otherwise) if (name %in% names(extra)) extra[[name]] else otherwise
  dialect <- with_options(dialect, given("skip", dialect$skip),
                          given("header_row", dialect$header_row), given("fill", dialect$fill),
                          given("long_rows", dialect$long_rows), given("na", dialect$na),
                          given("encoding", dialect$encoding))
  buffer_bytes <- given("buffer_bytes", 262144)
  if (inherits(source, "connection") && !dialect$has_header) {
    stop(paste("a source with no header is opened twice, once for its width: give a file path",
               "or a raw vector, not a connection"), call. = FALSE)
  }
  limit <- min(sample_rows, sample_cap)
  open <- function() delimited_reader(source, dialect, batch_rows = 1, buffer_bytes = buffer_bytes)
  width <- NULL
  if (!dialect$has_header) {
    first <- open()
    on.exit(close(first), add = TRUE)
    bind(first, list(col_text(1)))
    sample_read(first, FALSE)
    width <- .Call(C_delimited_width, first$ptr)
    close(first)
  }
  reader <- open()
  on.exit(close(reader), add = TRUE)
  width <- width %||% length(reader$header)
  sample_from(reader, width, limit, sheet = FALSE, asked)
}

suggest_sheet <- function(book, sheet, sample_rows, asked, ...) {
  extra <- list(...)
  unknown <- setdiff(names(extra), c("has_header", "skip_empty_rows", "batch_rows", "skip",
                                     "header_row", "na", "range"))
  if (length(unknown) || (length(extra) && is.null(names(extra)))) {
    stop(sprintf("suggest_plan() reads a sheet with no argument %s",
                 if (length(unknown)) unknown[[1]] else "unnamed"),
         call. = FALSE)
  }
  batch_rows <- count_arg(extra[["batch_rows"]] %||% 4096, "batch_rows")
  # The sheet's batch size is the core's to judge, a 0 refused, before it caps the sample (as a
  # Rust sheet's suggest stops at its own batch size).
  check_sizes(NULL, batch_rows, 1, batch_sheet)
  limit <- min(sample_rows, batch_rows)
  open <- function() {
    args <- extra
    args$batch_rows <- 1
    do.call(sheet_reader, c(list(book, sheet), args))
  }
  has_header <- extra[["has_header"]] %||% TRUE
  width <- NULL
  if (!isTRUE(has_header)) {
    first <- open()
    on.exit(close(first), add = TRUE)
    first$widths <- TRUE
    bind(first, list(col_text(1)))
    raw <- sample_read(first, FALSE)
    width <- if (is.null(raw)) 0 else raw$widths[[1]]
    close(first)
  }
  reader <- open()
  on.exit(close(reader), add = TRUE)
  width <- width %||% length(reader$header)
  sample_from(reader, width, limit, sheet = TRUE, asked)
}

# The sample a reader opened with no plan gives, and what the core makes of it.
sample_from <- function(reader, width, limit, sheet, asked) {
  if (!is.null(reader$failure)) {
    return(failed_suggestion(reader$failure))
  }
  if (width > max_position) {
    stop(sprintf("plan column %s cannot be honoured: the source has %s columns",
                 fmt(max_position + 1), fmt(width)), call. = FALSE)
  }
  header <- reader$header
  cells <- rep(list(list()), width)
  marks <- rep(list(raw()), width)
  rows <- 0
  bytes <- 0
  truncated <- FALSE
  if (width > 0) {
    plan <- lapply(seq_len(width), col_text)
    if (sheet) plan <- c(plan, lapply(seq_len(width), col_uuid))
    packed <- pack_specs(plan, rep(seq_len(width) - 1L, if (sheet) 2L else 1L))
    # One row more than the sample, so a row past it costs no second batch, in as many batches
    # as the core says that takes.
    said <- .Call(C_batch_check, packed$specs, packed$lists, limit + 1, 1, reader$flags)
    if (said[[5]] == 0) check_sizes(packed, limit + 1, 1, reader$flags)
    reader$batch_rows <- said[[5]]
    bind(reader, plan)
    repeat {
      raw <- tryCatch(sample_read(reader, TRUE), hypertabular_io_error = function(e) {
        if (rows < limit) stop(e)
        truncated <<- TRUE
        NULL
      })
      if (truncated) break
      if (is.null(raw)) {
        if (!is.null(reader$failure)) {
          if (rows < limit) return(failed_suggestion(reader$failure))
          truncated <- TRUE
        }
        break
      }
      raw <- raw$columns
      n <- length(raw[[1]])
      room <- limit - rows
      sizes <- Reduce(`+`, lapply(raw[seq_len(width)], lengths))
      fits <- which(bytes + cumsum(sizes) > sample_bytes)
      take <- min(n, room, if (length(fits)) fits[[1]] - 1 else n)
      for (k in seq_len(width)) {
        cells[[k]] <- c(cells[[k]], raw[[k]][seq_len(take)])
        if (sheet) marks[[k]] <- c(marks[[k]], raw[[width + k]][seq_len(take)])
      }
      rows <- rows + take
      bytes <- bytes + sum(sizes[seq_len(take)])
      if (take < n) {
        truncated <- TRUE
        break
      }
    }
  }
  judge_sample(cells, if (sheet) marks, rows, header, asked, truncated)
}

failed_suggestion <- function(failure) {
  structure(list(plan = NULL, report = NULL, truncated = FALSE, rows = 0, failure = failure),
            class = "hypertabular_suggestion")
}

# Asks the core, and builds the plan and the report from its answer.
judge_sample <- function(cells, marks, rows, header, asked, truncated) {
  width <- length(cells)
  names <- vapply(seq_len(width), function(k) {
    named <- !is.null(header) && k <= length(header) && !is.na(header[[k]])
    if (named) header[[k]] else NA_character_
  }, "")
  declared_formats <- lapply(seq_len(width), function(k) {
    declared_for(asked$column_formats, k - 1L, names[[k]], asked$format)
  })
  formats <- unlist(lapply(declared_formats, function(f) {
    if (is.null(f)) raw(format_bytes) else packed_format(f)
  }), use.names = FALSE) %||% raw()
  found <- .Call(C_suggest, cells, marks, rows, formats)
  # Every format was judged and every cell was made here.
  if (is.null(found)) stop("the core refuses the sample", call. = FALSE)
  columns <- lapply(seq_len(width), function(k) {
    order <- declared_for(asked$column_date_orders, k - 1L, names[[k]], asked$date_order)
    suggested_column(k, names[[k]], header, declared_formats[[k]], order, found[[1]][[k]],
                     found[[2]][[k]], found[[3]][[k]], found[[4]][[k]], found[[5]][[k]])
  })
  plan <- lapply(columns, `[[`, "plan")
  names(plan) <- repair_names(as.character(ifelse(is.na(names) | !nzchar(names),
                                                  paste0("V", seq_len(width)), names)))
  report <- data.frame(
    column = seq_len(width),
    name = names,
    door = vapply(columns, `[[`, "", "door"),
    param = vapply(columns, `[[`, 0, "param"),
    status = vapply(columns, `[[`, "", "status"),
    matched = found[[4]],
    sampled = found[[5]],
    stringsAsFactors = FALSE
  )
  report$candidates <- lapply(columns, `[[`, "candidates")
  report$format <- lapply(columns, `[[`, "format")
  structure(list(plan = plan, report = report, truncated = truncated, rows = rows,
                 failure = NULL),
            class = "hypertabular_suggestion")
}

# The source a suggested column names: the header's name where it names the column once,
# otherwise its position.
suggested_source <- function(k, name, header) {
  if (!is.na(name) && nzchar(name) && sum(header == name, na.rm = TRUE) == 1L) name else k
}

suggested_column <- function(k, name, header, format, order, door, param, status, matched,
                             sampled) {
  status <- if (status %in% 1:3) statuses[[status]] else "unresolved"
  door_name <- names(door_codes)[match(door, door_codes)]
  if (is.na(door_name)) door_name <- "text"
  dated <- door_name %in% c("date_ordered", "datetime")
  candidates <- list()
  said <- param
  if (status == "ambiguous") {
    if (dated) {
      candidates <- as.list(orders[bitwAnd(param, bitwShiftL(1L, 1:3)) != 0])
      said <- if (param == 0) 0 else log2(bitwAnd(param, -param))
    } else {
      candidates <- c(
        if (bitwAnd(param, 2L)) list(hypercast::num_format()),
        if (bitwAnd(param, 4L)) list(hypercast::num_format(",", "."))
      )
    }
  }
  if (!is.null(order) && status == "ambiguous" && dated && order %in% unlist(candidates)) {
    status <- "resolved"
    candidates <- list()
    door_name <- "date_ordered"
    said <- match(order, orders)
    param <- said
  }
  numeric <- status == "resolved" && door_name %in% numeric_doors
  planned <- if (numeric) {
    format %||% if (param == 4) hypercast::num_format(",", ".") else hypercast::num_format()
  }
  source <- suggested_source(k, name, header)
  plan <- if (status != "resolved") {
    col_text(source)
  } else {
    switch(door_name,
      date_ordered = col_date(source, orders[[said]]),
      datetime = col_datetime(source, orders[[said]]),
      text = col_text(source),
      # The invariant notation is what a numeric column reads with no format.
      if (numeric && !identical(planned$packed, hypercast::num_format()$packed)) {
        get(paste0("col_", door_name))(source, planned)
      } else {
        get(paste0("col_", door_name))(source)
      }
    )
  }
  # The door's parameter, as Rust's report and every other binding give it: a date's order (the
  # first candidate's where ambiguous), 0 for a number, whose notation is `format` or
  # `candidates`, and the core's own for the rest (a Unix precision, an Excel epoch).
  door_param <- if (dated) said else if (door_name %in% numeric_doors) 0 else param
  list(plan = plan, door = door_name, param = door_param, status = status,
       candidates = candidates, format = planned)
}

#' @rdname suggest_plan
#' @export
print.hypertabular_suggestion <- function(x, ...) {
  if (!is.null(x$failure)) {
    cat("# the sample failed: ", format(x$failure), "\n", sep = "")
    return(invisible(x))
  }
  cat(sprintf("# %d column%s from a sample of %s row%s%s\n", length(x$plan),
              if (length(x$plan) == 1L) "" else "s", fmt(x$rows), if (x$rows == 1) "" else "s",
              if (x$truncated) ", the source has more" else ""))
  if (!length(x$plan)) {
    cat("list()\n")
    return(invisible(x))
  }
  calls <- vapply(seq_along(x$plan), function(k) {
    name <- names(x$plan)[[k]]
    if (!identical(make.names(name), name)) name <- encodeString(name, quote = "`")
    sprintf("  %s = %s%s", name,
            column_call(x$plan[[k]]), if (k < length(x$plan)) "," else "")
  }, "")
  notes <- vapply(seq_len(nrow(x$report)), function(k) {
    r <- x$report[k, ]
    what <- switch(r$status,
      ambiguous = paste0("ambiguous, ", r$door, ": ", paste(vapply(r$candidates[[1]], function(c) {
        if (is.character(c)) {
          encodeString(c, quote = "\"")
        } else {
          trimws(utils::capture.output(print(c)))
        }
      }, ""), collapse = " or ")),
      r$status
    )
    sprintf("# %s, %s of %s", what, fmt(r$matched), fmt(r$sampled))
  }, "")
  pad <- max(nchar(calls))
  cat("list(\n", paste0(formatC(calls, width = -pad), "  ", notes, collapse = "\n"), "\n)\n",
      sep = "")
  invisible(x)
}
