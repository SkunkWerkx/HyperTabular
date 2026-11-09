long_rows_policies <- c("reject", "report", "truncate")
# The encodings a dialect can declare, by the core's hypertabular_transcode code (0 is UTF-8,
# read in place).
encodings <- c("utf-8" = 0L, "windows-1252" = 1L, "latin-1" = 2L, "utf-16le" = 3L)

flag_arg <- function(value, what) {
  if (!is.logical(value) || length(value) != 1L || is.na(value)) {
    stop(sprintf("%s must be TRUE or FALSE", what), call. = FALSE)
  }
  value
}

# A count a u32 field of the core holds.
u32_arg <- function(value, what) {
  if (!is.numeric(value) || is.object(value) || length(value) != 1L || is.na(value) ||
        value < 0 || value > 4294967295 || value != floor(value)) {
    stop(sprintf("%s must be one whole number from 0 to 2^32 - 1", what), call. = FALSE)
  }
  as.numeric(value)
}

#' Delimited dialects
#'
#' How delimited text is cut into cells, declared rather than sniffed: the separator, whether
#' `"` quotes cells (RFC 4180, `""` for a literal quote), whether the first record is a
#' header, whether a completely empty line is skipped rather than read as a row of one empty
#' cell, what comes before the header, what a record of another width is, what stands for
#' no value, and what encoding the bytes are in. Rows end at `\n`, `\r\n` or `\r`; a UTF-8
#' byte-order mark at the start is skipped. The header, or else the first record, fixes the
#' width every later record is held to.
#'
#' @param separator One byte: tab, or printable ASCII other than `"`.
#' @param quoting Whether `"` quotes cells.
#' @param has_header Whether the first record read is a header: read when the reader opens,
#'   never delivered as a row.
#' @param skip_blank_lines Whether a completely empty line is skipped.
#' @param skip Physical lines to pass over after a byte-order mark, before anything is read of
#'   them, so a quote in one means nothing.
#' @param header_row Records to read and discard after those, ahead of the header if there is
#'   one. A blank line the dialect skips isn't a record.
#' @param fill What a record with fewer cells than the width is: `FALSE`, a
#'   `"column_count"` failure; `TRUE`, a row whose missing cells are `empty` verdicts at origin
#'   `"padded"`.
#' @param long_rows What a record with more cells than the width is: `"reject"`, a
#'   `"column_count"` failure; `"truncate"`, a row cut to the width; `"report"`, a row cut to
#'   the width that [problems()] lists, with the record's own width.
#' @param na The cells every column reads as missing, unless it declares its own: an exact,
#'   unquoted match of the cell's bytes is an `empty` verdict at origin `"token"`.
#'   `character(0)` declares none.
#' @param encoding What the input's bytes are: `"utf-8"`, read in place, or
#'   `"windows-1252"`, `"latin-1"` or `"utf-16le"`, decoded to UTF-8 ahead of the read. Never
#'   sniffed. A unit the encoding has no character for ends the read in an `"encoding"`
#'   failure, after every row before it.
#' @return A `hypertabular_dialect`.
#' @family reads
#' @export
#' @examples
#' delimited_dialect()
#' delimited_dialect("\t", has_header = FALSE)
#' delimited_dialect(skip = 2, fill = TRUE, na = c("NA", "-"), encoding = "windows-1252")
delimited_dialect <- function(separator = ",", quoting = TRUE, has_header = TRUE,
                              skip_blank_lines = TRUE, skip = 0, header_row = 0, fill = FALSE,
                              long_rows = "reject", na = character(), encoding = "utf-8") {
  byte <- if (is.character(separator) && length(separator) == 1L && !is.na(separator)) {
    charToRaw(separator)
  }
  if (length(byte) != 1L || !(byte == as.raw(9L) || (byte >= as.raw(0x20) &&
                                                       byte <= as.raw(0x7e) &&
                                                       byte != as.raw(0x22)))) {
    stop("separator must be one byte: tab, or printable ASCII other than '\"'", call. = FALSE)
  }
  declared(long_rows, long_rows_policies, "long_rows")
  declared(encoding, names(encodings), "encoding")
  structure(
    list(separator = separator, quoting = flag_arg(quoting, "quoting"),
         has_header = flag_arg(has_header, "has_header"),
         skip_blank_lines = flag_arg(skip_blank_lines, "skip_blank_lines"),
         skip = u32_arg(skip, "skip"), header_row = u32_arg(header_row, "header_row"),
         fill = flag_arg(fill, "fill"), long_rows = long_rows, na = check_tokens(na, FALSE),
         encoding = encoding),
    class = "hypertabular_dialect"
  )
}

#' @export
print.hypertabular_dialect <- function(x, ...) {
  extra <- c(
    if (x$skip != 0) paste("skip =", fmt(x$skip)),
    if (x$header_row != 0) paste("header_row =", fmt(x$header_row)),
    if (x$fill) "fill = TRUE",
    if (x$long_rows != "reject") paste0("long_rows = \"", x$long_rows, "\""),
    if (length(x$na)) {
      paste0("na = c(", paste(encodeString(x$na, quote = "\""), collapse = ", "), ")")
    },
    if (x$encoding != "utf-8") paste0("encoding = \"", x$encoding, "\"")
  )
  cat(sprintf("delimited_dialect(%s)\n", paste(c(
    encodeString(x$separator, quote = "\""), paste("quoting =", x$quoting),
    paste("has_header =", x$has_header), paste("skip_blank_lines =", x$skip_blank_lines), extra
  ), collapse = ", ")))
  invisible(x)
}

check_dialect <- function(dialect) {
  if (!inherits(dialect, "hypertabular_dialect")) {
    stop("dialect must be a hypertabular_dialect, as delimited_dialect() returns", call. = FALSE)
  }
  dialect
}

# The dialect with what the call declares beside it.
with_options <- function(dialect, skip, header_row, fill, long_rows, na, encoding) {
  dialect <- check_dialect(dialect)
  delimited_dialect(dialect$separator, dialect$quoting, dialect$has_header,
                    dialect$skip_blank_lines, skip, header_row, fill, long_rows, na, encoding)
}

# The 24 bytes of the core's RawDelimitedOptions, little-endian.
packed_options <- function(dialect) {
  flags <- as.raw(c(dialect$quoting, dialect$skip_blank_lines, 0L))
  c(u32_le(24), charToRaw(dialect$separator), flags,
    u32_le(dialect$skip), u32_le(dialect$header_row), u32_le(if (dialect$fill) 1 else 0),
    u32_le(match(dialect$long_rows, long_rows_policies) - 1L))
}

# A whole number the core takes as a u64: R checks only that it is one.
count_arg <- function(value, what) {
  if (!is.numeric(value) || is.object(value) || length(value) != 1L || is.na(value) ||
        value < 0 || value != floor(value) || value >= 2^64) {
    stop(sprintf("%s must be one whole number from 0", what), call. = FALSE)
  }
  as.numeric(value)
}

# The core's hypertabular_batch_check flags.
batch_notes <- 1L
batch_widths <- 2L
batch_sheet <- 4L

# Asks the core whether a read may ask for this much, before anything is allocated for it: the
# core's answer, c(code, needed, limit, buffer_bytes, sample_rows, width, refused), or an R error
# in the core's terms. `packed` is pack_specs()'s, or none for a read whose plan comes later.
check_sizes <- function(packed, batch_rows, buffer_bytes, flags) {
  packed <- packed %||% list(specs = raw(), lists = list())
  out <- .Call(C_batch_check, packed$specs, packed$lists, batch_rows, buffer_bytes, flags)
  if (out[[1]] == 0) {
    return(out)
  }
  exact <- attr(out, "exact")
  said <- switch(as.character(out[[7]]),
                 "1" = "batch_rows must be at least 1",
                 "2" = "buffer_bytes must be at least 1",
                 "4" = sprintf("a row of %s columns asks for %s bytes, past the limit of %s",
                               fmt(out[[6]]), exact[[1]], exact[[2]]),
                 sprintf("batch_rows asks for %s bytes, past the limit of %s", exact[[1]],
                         exact[[2]]))
  stop(said, call. = FALSE)
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
#' `hypertabular_table`. [close()][close.hypertabular_reader] lets go of a reader early.
#'
#' The input is bytes: a raw vector, a file path, or a connection (opened in binary if it is
#' not open; a connection the reader opens it also closes, once the read ends or the reader is
#' closed). It is read as UTF-8 unless the dialect declares another encoding, and is read
#' through `buffer_bytes` at a time; a record longer than the buffer grows it, up to a 1 GiB
#' ceiling past which the record is a `"row_too_long"` failure. Nothing about the input is
#' sniffed: dialect, plan, number formats and date orders are all declared.
#'
#' Every cell is a HyperCast verdict, and a file that is structurally broken is a
#' [hypertabular_failure] handed back as a value, after every intact row before it: no read
#' signals an R error for anything in its data. R errors are reserved for the caller's own
#' declarations (a dialect, a plan or a size that cannot be, a source column the header does
#' not have or has twice, a plan bound twice or a read before one is bound), for a read of a
#' closed reader, for a read from inside one already under way on the same reader (from an
#' interrupt or condition handler, say), and for errors the connection itself raises.
#'
#' The sizes are the core's to judge: `batch_rows` or `buffer_bytes` of 0, or a batch whose
#' arrays would pass what the platform can address, is an R error in its words, before anything
#' is allocated.
#'
#' @param source The input: a raw vector, a file path, or a connection.
#' @param dialect A [delimited_dialect()].
#' @param plan A list of columns ([col_bool()] and its siblings), named for the output
#'   columns; or `NULL`, to bind one later. For `read_delimited()`, `NULL` reads every header
#'   column through [col_text()].
#' @param batch_rows The most rows a batch holds.
#' @param buffer_bytes The bytes read from a file or connection at a time.
#' @param skip,header_row,fill,long_rows,na,encoding As for [delimited_dialect()], in place of
#'   the dialect's own.
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
#' close(reader)
#'
#' read_delimited(csv, list(col_i32(1), col_text(3)))$columns
#' read_delimited(charToRaw("exported by a tool\nid\n1\nNA\n"), list(col_i32("id")),
#'                skip = 1, na = "NA")$columns
delimited_reader <- function(source, dialect = delimited_dialect(), plan = NULL,
                             batch_rows = 4096, buffer_bytes = 262144, skip = dialect$skip,
                             header_row = dialect$header_row, fill = dialect$fill,
                             long_rows = dialect$long_rows, na = dialect$na,
                             encoding = dialect$encoding) {
  dialect <- with_options(dialect, skip, header_row, fill, long_rows, na, encoding)
  batch_rows <- count_arg(batch_rows, "batch_rows")
  buffer_bytes <- count_arg(buffer_bytes, "buffer_bytes")
  widths <- dialect$long_rows == "report"
  flags <- batch_notes + if (widths) batch_widths else 0L
  sizes <- check_sizes(NULL, batch_rows, buffer_bytes, flags)
  if (!is.null(plan)) {
    plan <- as_plan(plan)
    # What the plan declares is judged before the source is opened; where it reads is judged
    # at bind, against the header.
    judge_plan(plan, pack_specs(plan, integer(length(plan))))
  }
  if (!is.raw(source) && !inherits(source, "connection") &&
        !(is.character(source) && length(source) == 1L && !is.na(source))) {
    stop("source must be a raw vector, a file path or a connection", call. = FALSE)
  }
  reader <- new_reader(c("hypertabular_delimited_reader", "hypertabular_reader"), batch_rows)
  reader$flags <- flags
  # A connection's read is a whole number of bytes R can ask readBin for at once.
  reader$chunk <- min(sizes[[4]], .Machine$integer.max)
  reader$dialect <- dialect
  reader$long_rows <- match(dialect$long_rows, long_rows_policies) - 1L
  reader$widths <- widths
  opened <- FALSE
  # A reader that doesn't finish opening (an error, an interrupt) closes what it opened.
  on.exit(if (!opened) close_source(reader), add = TRUE)
  reg.finalizer(reader, close_source, onexit = TRUE)
  if (inherits(source, "connection")) {
    if (isOpen(source) && !identical(summary(source)$text, "binary")) {
      stop(paste("a connection must be opened in binary mode (\"rb\"), or left closed to be",
                 "opened so: for standard input, file(\"stdin\"); for text in memory, a raw",
                 "vector (charToRaw())"), call. = FALSE)
    }
    reader$con <- source
    if (!isOpen(source)) {
      open(source, "rb")
      reader$own <- TRUE
    }
  } else if (!is.raw(source)) {
    reader$con <- file(source, "rb")
    reader$own <- TRUE
  }
  reader$whole <- !is.null(reader$con) && whole_reads(reader$con)
  reader$ptr <- .Call(C_delimited_new, packed_options(dialect),
                      if (length(dialect$na)) packed_tokens(dialect$na),
                      encodings[[dialect$encoding]], stingy())
  if (is.raw(source)) {
    .Call(C_delimited_feed, reader$ptr, source)
    .Call(C_delimited_feed, reader$ptr, NULL)
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
  opened <- TRUE
  if (!is.null(plan)) bind(reader, plan)
  reader
}

# Connections whose read is bounded and local: a raw connection, and a file, compressed or not,
# that is a regular file (not a FIFO, a device or stdin, whose read can wait indefinitely).
whole_reads <- function(con) {
  about <- summary(con)
  if (identical(about$class, "rawConnection")) return(TRUE)
  about$class %in% c("file", "gzfile", "bzfile", "xzfile") &&
    !identical(about$description, "stdin") &&
    .Call(C_regular_file, about$description)
}

# More input for the reader: at least as much as it already holds unread, or its end. The shim
# calls readBin and feeds what it returns in one .Call, so no interrupt lands between the two. A
# bounded local read (`whole`) runs with interrupts held, so none lands inside it either; any
# other connection's read can wait on the outside world and stays interruptible, and an interrupt
# inside it leaves the reader refusing, since bytes may have been taken.
# An error the connection raises is classed hypertabular_io_error beside its own classes; one
# the caller asked for (a warning options(warn = 2) made an error) and a connection closed under
# the reader are the caller's, and keep their own.
feed <- function(reader) {
  con <- reader$con
  if (is.null(con)) {
    .Call(C_delimited_feed, reader$ptr, NULL)
    return(invisible())
  }
  converted <- FALSE
  pull <- function() {
    .Call(C_delimited_pull, reader$ptr, con, reader$chunk, reader$whole, environment())
  }
  ended <- withCallingHandlers(
    if (reader$whole) suspendInterrupts(pull()) else interruptible(reader, pull()),
    warning = function(w) {
      if (getOption("warn") >= 2) converted <<- TRUE
    },
    error = function(e) {
      # The caller's: a warning options(warn = 2) made an error, or the shim's own refusal.
      if (converted || identical(conditionCall(e)[[1]], quote(.Call))) {
        return()
      }
      if (!isTRUE(tryCatch(isOpen(con), error = function(e) FALSE))) {
        stop("the reader's connection was closed while the reader was reading it", call. = FALSE)
      }
      class(e) <- unique(c("hypertabular_io_error", class(e)))
      stop(e)
    }
  )
  if (is.na(ended)) {
    stop(paste("an interrupted read can't resume from here: the connection's read was",
               "interrupted part-way, so bytes may be missing; close() the reader and open the",
               "source again"), call. = FALSE)
  }
  if (ended) close_source(reader)
  invisible()
}

bind_delimited <- function(reader, plan, packed_plan) {
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
  packed <- pack_specs(plan, resolved$ordinals)
  judge_plan(plan, packed)
  check_sizes(packed, reader$batch_rows, 1, reader$flags)
  per_row <- max(c(resolved$ordinals, -1L)) + 2L
  .Call(C_delimited_bind, reader$ptr, packed$specs, packed$lists, packed$raws, reader$batch_rows,
        per_row)
  resolved
}

# The next batch as the C shim wrote it, or NULL at the end or a failure.
read_delimited_batch <- function(reader, sample = FALSE) {
  repeat {
    res <- interruptible(reader, .Call(C_delimited_fill, reader$ptr, sample))
    switch(res$status + 1L,
      return(structure(res$payload, first = attr(res, "first"))),
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
                           batch_rows = 4096, buffer_bytes = 262144, skip = dialect$skip,
                           header_row = dialect$header_row, fill = dialect$fill,
                           long_rows = dialect$long_rows, na = dialect$na,
                           encoding = dialect$encoding) {
  reader <- delimited_reader(source, dialect, batch_rows = batch_rows,
                             buffer_bytes = buffer_bytes, skip = skip, header_row = header_row,
                             fill = fill, long_rows = long_rows, na = na, encoding = encoding)
  on.exit(close(reader), add = TRUE)
  read_all(reader, plan)
}
