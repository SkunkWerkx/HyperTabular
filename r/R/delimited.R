#' Delimited dialects
#'
#' How delimited text is cut into cells, declared rather than sniffed: the separator, whether
#' `"` quotes cells (RFC 4180, `""` for a literal quote), whether the first record is a
#' header, and whether a completely empty line is skipped rather than read as a row of one
#' empty cell. Rows end at `\n`, `\r\n` or `\r`; a UTF-8 byte-order mark at the start is
#' skipped. The first record fixes the number of cells every later one must have.
#'
#' @param separator One byte: tab, or printable ASCII other than `"`.
#' @param quoting Whether `"` quotes cells.
#' @param has_header Whether the first record is a header: read when the reader opens, never
#'   delivered as a row.
#' @param skip_blank_lines Whether a completely empty line is skipped.
#' @return A `hypertabular_dialect`.
#' @family reads
#' @export
#' @examples
#' delimited_dialect()
#' delimited_dialect("\t", has_header = FALSE)
delimited_dialect <- function(separator = ",", quoting = TRUE, has_header = TRUE,
                              skip_blank_lines = TRUE) {
  byte <- if (is.character(separator) && length(separator) == 1L && !is.na(separator)) {
    charToRaw(separator)
  }
  if (length(byte) != 1L || !(byte == as.raw(9L) || (byte >= as.raw(0x20) &&
                                                       byte <= as.raw(0x7e) &&
                                                       byte != as.raw(0x22)))) {
    stop("separator must be one byte: tab, or printable ASCII other than '\"'", call. = FALSE)
  }
  flag <- function(value, what) {
    if (!is.logical(value) || length(value) != 1L || is.na(value)) {
      stop(sprintf("%s must be TRUE or FALSE", what), call. = FALSE)
    }
    value
  }
  structure(
    list(separator = separator, quoting = flag(quoting, "quoting"),
         has_header = flag(has_header, "has_header"),
         skip_blank_lines = flag(skip_blank_lines, "skip_blank_lines")),
    class = "hypertabular_dialect"
  )
}

#' @export
print.hypertabular_dialect <- function(x, ...) {
  cat(sprintf("delimited_dialect(%s, quoting = %s, has_header = %s, skip_blank_lines = %s)\n",
              encodeString(x$separator, quote = "\""), x$quoting, x$has_header,
              x$skip_blank_lines))
  invisible(x)
}

check_dialect <- function(dialect) {
  if (!inherits(dialect, "hypertabular_dialect")) {
    stop("dialect must be a hypertabular_dialect, as delimited_dialect() returns", call. = FALSE)
  }
  as.raw(c(as.integer(charToRaw(dialect$separator)), dialect$quoting, dialect$skip_blank_lines, 0L))
}

count_arg <- function(value, what) {
  if (!is.numeric(value) || length(value) != 1L || is.na(value) || value < 1 ||
        value != floor(value) || value > .Machine$integer.max) {
    stop(sprintf("%s must be one positive whole number", what), call. = FALSE)
  }
  as.integer(value)
}

# The test suite's switch: every buffer a call works in starts at one element, so every
# call that can stop for room does.
stingy <- function() isTRUE(getOption("hypertabular.stingy"))

#' Read delimited text
#'
#' `delimited_reader()` opens delimited text for a forward-only read, a batch at a time:
#' [read_batch()] returns each [hypertabular_batch] in turn, and `NULL` once there are no more.
#' With a header declared, the header is read as the reader opens ([header()]); the plan, given
#' here or later through [bind()], may then name its source columns by header name.
#' `read_delimited()` reads the whole input through a plan and returns it as one
#' `hypertabular_table`.
#'
#' The input is bytes, read as UTF-8 and never re-encoded: a raw vector, a file path, or a
#' connection (opened in binary if it is not open; a connection the reader opens it also
#' closes, once the read ends). It is read through `buffer_bytes` at a time; a record longer
#' than the buffer grows it, up to a 1 GiB ceiling past which the record is a
#' `"row_too_long"` failure. Nothing about the input is sniffed: dialect, plan, number formats
#' and date orders are all declared.
#'
#' Every cell is a HyperCast verdict, and a file that is structurally broken is a
#' [hypertabular_failure] handed back as a value, after every intact row before it: no read
#' signals an R error for anything in its data. R errors are reserved for the caller's own
#' declarations: a dialect, a plan or a size that cannot be, a source column the header does
#' not have, a plan bound twice or a read before one is bound.
#'
#' @param source The input: a raw vector, a file path, or a connection.
#' @param dialect A [delimited_dialect()].
#' @param plan A list of columns ([col_bool()] and its siblings), named for the output
#'   columns; or `NULL`, to bind one later. For `read_delimited()`, `NULL` reads every header
#'   column through [col_text()].
#' @param batch_rows The most rows a batch holds.
#' @param buffer_bytes The bytes read from a file or connection at a time.
#' @return `delimited_reader()` returns a `hypertabular_delimited_reader`;
#'   `read_delimited()` a `hypertabular_table`.
#' @family reads
#' @export
#' @examples
#' csv <- charToRaw("id,when,amount\n1,7/1/2026,\"1,234.5\"\n2,13/1/2026,12x\n")
#' reader <- delimited_reader(csv)
#' header(reader)
#' bind(reader, list(id = col_i32("id"), when = col_date("when", "month_day_year"),
#'                   amount = col_f64("amount")))
#' batch <- read_batch(reader)
#' batch$columns$when
#' read_batch(reader)
#' failure(reader)
#'
#' read_delimited(csv, list(col_i32(1), col_text(3)))$columns
delimited_reader <- function(source, dialect = delimited_dialect(), plan = NULL,
                             batch_rows = 4096L, buffer_bytes = 262144L) {
  packed <- check_dialect(dialect)
  reader <- new.env(parent = emptyenv())
  reader$batch_rows <- count_arg(batch_rows, "batch_rows")
  reader$buffer_bytes <- count_arg(buffer_bytes, "buffer_bytes")
  if (!is.null(plan)) plan <- as_plan(plan)
  reader$con <- NULL
  reader$own <- FALSE
  if (is.raw(source)) {
    input <- source
  } else if (inherits(source, "connection")) {
    reader$con <- source
    if (!isOpen(source)) {
      open(source, "rb")
      reader$own <- TRUE
    }
  } else if (is.character(source) && length(source) == 1L && !is.na(source)) {
    reader$con <- file(source, "rb")
    reader$own <- TRUE
  } else {
    stop("source must be a raw vector, a file path or a connection", call. = FALSE)
  }
  reader$ptr <- .Call(C_delimited_new, packed, stingy())
  reader$unread <- 0
  reader$pending <- NULL
  reader$header <- NULL
  reader$failure <- NULL
  reader$done <- FALSE
  reader$plan <- NULL
  reader$names <- character()
  reader$dialect <- dialect
  class(reader) <- c("hypertabular_delimited_reader", "hypertabular_reader")
  if (is.raw(source)) {
    .Call(C_delimited_feed, reader$ptr, input)
    .Call(C_delimited_feed, reader$ptr, NULL)
  } else {
    reg.finalizer(reader, close_source, onexit = TRUE)
  }
  if (dialect$has_header) {
    repeat {
      res <- .Call(C_delimited_header, reader$ptr)
      if (res$status == 1L) {
        feed(reader)
        next
      }
      if (res$status == 3L) {
        end_read(reader, new_failure(res$failure))
      } else {
        reader$header <- res$payload
      }
      break
    }
  }
  if (!is.null(plan)) bind(reader, plan)
  reader
}

# More input for the reader: at least as much as it already holds unread, or its end.
feed <- function(reader) {
  con <- reader$con
  chunk <- if (!is.null(con)) readBin(con, "raw", max(reader$buffer_bytes, reader$unread))
  if (!length(chunk)) {
    .Call(C_delimited_feed, reader$ptr, NULL)
    close_source(reader)
  } else {
    reader$unread <- .Call(C_delimited_feed, reader$ptr, chunk)
  }
}

close_source <- function(reader) {
  if (isTRUE(reader$own) && !is.null(reader$con)) {
    try(close(reader$con), silent = TRUE)
  }
  reader$con <- NULL
  reader$own <- FALSE
}

# The read is over: its failure, if it ended on one, is final.
end_read <- function(reader, failure = NULL) {
  reader$done <- TRUE
  if (!is.null(failure)) reader$failure <- failure
  close_source(reader)
}

bind_delimited <- function(reader, plan) {
  header <- reader$header
  resolved <- resolve_plan(plan, header)
  width <- if (!is.null(header)) .Call(C_delimited_width, reader$ptr) else 0
  if (width > 0) {
    past <- which(resolved$ordinals >= width)
    if (length(past)) {
      stop(sprintf(paste("plan column %d reads source column %d, past the %d columns the",
                         "header gives every record"),
                   past[[1]], resolved$ordinals[[past[[1]]]] + 1L, as.integer(width)),
           call. = FALSE)
    }
  }
  per_row <- max(c(resolved$ordinals, -1L)) + 2L
  .Call(C_delimited_bind, reader$ptr, pack_plan(plan, resolved$ordinals), reader$batch_rows,
        per_row)
  reader$plan <- plan
  reader$names <- resolved$names
}

read_delimited_batch <- function(reader) {
  repeat {
    res <- .Call(C_delimited_fill, reader$ptr)
    switch(res$status + 1L,
      return(res$payload),
      feed(reader),
      {
        end_read(reader)
        return(NULL)
      },
      {
        end_read(reader, new_failure(res$failure))
        return(NULL)
      }
    )
  }
}

#' @rdname delimited_reader
#' @export
read_delimited <- function(source, plan = NULL, dialect = delimited_dialect(),
                           batch_rows = 4096L, buffer_bytes = 262144L) {
  reader <- delimited_reader(source, dialect, batch_rows = batch_rows,
                             buffer_bytes = buffer_bytes)
  on.exit(close_source(reader), add = TRUE)
  read_all(reader, plan)
}
