# The doors a plan column can be read through, by the core's ABI code; what a door declares
# beside itself is numbered as HyperCast numbers it.
door_codes <- c(
  bool = 1L, i8 = 2L, i16 = 3L, i32 = 4L, i64 = 5L, u8 = 6L, u16 = 7L, u32 = 8L, u64 = 9L,
  f32 = 10L, f64 = 11L, uuid = 12L, timestamp = 13L, unix = 14L, date = 15L, time = 16L,
  duration = 17L, text = 18L, decimal = 19L, date_ordered = 20L, datetime = 21L,
  excel_serial = 22L
)
numeric_doors <- c("i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
                   "decimal")
precisions <- c("seconds", "milliseconds", "microseconds", "nanoseconds")
orders <- c("year_month_day", "month_day_year", "day_month_year")
epochs <- c("y1900", "y1904")
# The widest source position a plan may name: the core's ordinal is a u32. Whether a read may
# reach that far is the core's to say (hypertabular_batch_check), before anything is allocated.
max_position <- 2^32
# The bytes of HyperCast's packed layouts this package hands the core: a number format's and
# the date options'. The core reads what their size words say, and refuses a size it doesn't.
format_bytes <- 92L
date_bytes <- 12L

declared <- function(value, choices, what) {
  if (!is.character(value) || length(value) != 1L || is.na(value) || !value %in% choices) {
    stop(sprintf("%s must be one of %s", what, paste0("\"", choices, "\"", collapse = ", ")),
         call. = FALSE)
  }
  match(value, choices)
}

check_source <- function(source) {
  ok <- (is.character(source) && length(source) == 1L && !is.na(source) && nzchar(source)) ||
    (is.numeric(source) && !is.object(source) && length(source) == 1L && !is.na(source) &&
       source >= 1 && source <= max_position && source == floor(source))
  if (!ok) {
    stop("source must be one column name, or one whole-number position from 1",
         call. = FALSE)
  }
  if (!is.numeric(source)) {
    return(source)
  }
  # An integer where R has one; a position past R's integers stays a whole double.
  if (source <= .Machine$integer.max) as.integer(source) else as.numeric(source)
}

# `bytes` as exactly `n` bytes, its size word kept: what a hypercast that packs another size of
# layout gave, for the core to judge by that word. Zeros pad a shorter one.
sized <- function(bytes, n) {
  if (length(bytes) >= n) bytes[seq_len(n)] else c(bytes, raw(n - length(bytes)))
}

# The size word of a packed layout, or NA for one too short to hold it.
size_word <- function(bytes) {
  if (length(bytes) < 4L) return(NA_real_)
  sum(as.numeric(bytes[1:4]) * 256^(0:3))
}

# The 92 bytes of HyperCast's RawNumFormat the core reads a numeric column with: the
# format's own (num_format()'s `packed`), or for none the invariant notation's: hypercast's,
# Western grouping, which all zeros would read too.
packed_format <- function(format) {
  if (is.null(format)) {
    return(hypercast::num_format()$packed)
  }
  if (!inherits(format, "hypercast_num_format") || !is.raw(format$packed)) {
    stop("format must be a hypercast_num_format, as hypercast::num_format() returns, or NULL",
         call. = FALSE)
  }
  sized(format$packed, format_bytes)
}

# The 12 bytes of HyperCast's RawDateOptions, or twelve zeros (a size of 0) for none.
packed_date <- function(options) {
  if (is.null(options)) raw(date_bytes) else sized(options$packed, date_bytes)
}

check_options <- function(options) {
  if (!is.null(options) &&
        (!inherits(options, "hypercast_date_options") || !is.raw(options$packed))) {
    stop("options must be a hypercast_date_options, as hypercast::date_options() returns, or NULL",
         call. = FALSE)
  }
  options
}

check_months <- function(months) {
  if (!is.null(months) &&
        (!inherits(months, "hypercast_month_names") || !is.raw(months$packed))) {
    stop("months must be a hypercast_month_names, as hypercast::month_names() returns, or NULL",
         call. = FALSE)
  }
  months
}

# `na` as the UTF-8 tokens the core compares a cell's exact, unquoted bytes with, or NULL where
# NULL is allowed (a column inheriting its read's).
check_tokens <- function(na, null_ok) {
  if (is.null(na) && null_ok) {
    return(NULL)
  }
  if (!is.character(na) || anyNA(na)) {
    stop(if (null_ok) "na must be a character vector without NA, or NULL to inherit the read's"
         else "na must be a character vector without NA", call. = FALSE)
  }
  enc2utf8(as.vector(na))
}

# Tokens as the core takes them: each a little-endian u32 length, then its bytes.
packed_tokens <- function(tokens) {
  if (is.null(tokens)) {
    return(NULL)
  }
  unlist(lapply(tokens, function(token) {
    bytes <- charToRaw(token)
    c(u32_le(length(bytes)), bytes)
  }), use.names = FALSE) %||% raw()
}

new_column <- function(source, door, param = 0L, format = NULL, factory = door, na = NULL,
                       options = NULL, months = NULL, raw = FALSE) {
  if (!is.logical(raw) || length(raw) != 1L || is.na(raw)) {
    stop("raw must be TRUE or FALSE", call. = FALSE)
  }
  structure(
    list(source = check_source(source), door = door, param = param,
         format = if (door %in% numeric_doors) packed_format(format) else raw(format_bytes),
         num_format = format, factory = factory, na = check_tokens(na, TRUE),
         options = check_options(options), months = check_months(months), raw = raw),
    class = "hypertabular_column"
  )
}

#' Plan columns
#'
#' A plan says which source columns to read and through which HyperCast door: a list of
#' columns, in the order to produce them, each naming its source column and its door. A plan
#' is a projection: a 40-column file can be read into 5 typed columns, and one source column
#' through two doors. Nothing is sniffed; every door, and everything it reads by, is declared.
#'
#' Each factory reads its source through the HyperCast door of the same name, as
#' `hypercast::cast_*()` reads text, and gives back what that door gives back (see
#' [hypercast::hypercast_verdict]): `col_bool()` through `col_u64()`, `col_f32()`,
#' `col_f64()` and `col_decimal()` the numbers, under a declared [hypercast::num_format()]
#' (`NULL`, the default, is the invariant notation, Western groups of three); `col_uuid()`;
#' `col_timestamp()`, `col_unix()` and `col_excel_serial()` the instants; `col_date()`,
#' `col_datetime()`, `col_time()` and `col_duration()` the civil values. `col_text()` asks for
#' the cell's text itself, cast by nothing: its value is the text, and only an empty cell is a
#' fault.
#'
#' A format is declared, never guessed: `1,23,456` is refused under the default, and read
#' under `hypercast::num_format_with_grouping(hypercast::num_format(), "indian")` declared
#' for that column, while a Western column beside it reads `1,234,567`.
#'
#' A workbook's typed cells (numbers, dates, booleans, errors) go through the door as the
#' cast matrix in the project's `docs/design.md` says: a stored number through the door's
#' typed twin, never through text, and a typed cell through `col_text()` as its canonical
#' text.
#'
#' Everything a column declares is judged by the core before anything is read: a plan the
#' core can't honour is an R error naming the column. A format or date options packed by a
#' hypercast whose layouts this hypertabular's core doesn't read is such an error, naming both
#' packages' versions.
#'
#' @param source The source column: its name in the header (matched exactly; a name the
#'   header holds twice is an error naming both positions) or its position, counted from 1. A
#'   position past the end of a row reads as an empty cell.
#' @param format A [hypercast::num_format()], or `NULL` for the invariant notation.
#' @param precision The Unix unit: `"seconds"`, `"milliseconds"`, `"microseconds"` or
#'   `"nanoseconds"`.
#' @param epoch The date system: `"y1900"` or `"y1904"`. A workbook says which it uses
#'   (`workbook()$date_system`); this door reads cells as the plan declares.
#' @param order The field order: `"year_month_day"`, `"month_day_year"` or
#'   `"day_month_year"`. `NULL`, for `col_date()`, reads strict ISO only.
#' @param options A [hypercast::date_options()], or `NULL` for none: the forms the door reads
#'   beyond its own, and a two-digit year's pivot.
#' @param months A [hypercast::month_names()], or `NULL`: a vocabulary of month names the door
#'   reads beside the English ones.
#' @param na The cells this column reads as missing (an exact, unquoted match of the cell's
#'   bytes, an `empty` verdict at origin `"token"`), in place of the read's own `na`; `NULL`
#'   inherits the read's, and `character(0)` declares none.
#' @param raw For `col_text()`, `TRUE` gives each value as a raw vector of the exact bytes
#'   (`NULL` where the cell faulted), which holds what an R string can't: a NUL, or more than
#'   `INT_MAX` bytes.
#' @return A `hypertabular_column`.
#' @family plans
#' @export
#' @examples
#' plan <- list(
#'   id = col_i32("id"),
#'   amount = col_decimal("amount", hypercast::num_format(",", ".")),
#'   when = col_date(3, order = "day_month_year"),
#'   note = col_text("note", na = c("", "-"))
#' )
#' plan$id
#'
#' # A column of Indian-grouped numbers, declared as such.
#' col_i64("lakhs", hypercast::num_format_with_grouping(hypercast::num_format(), "indian"))
col_bool <- function(source, na = NULL) new_column(source, "bool", na = na)

#' @rdname col_bool
#' @export
col_i8 <- function(source, format = NULL, na = NULL) {
  new_column(source, "i8", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_i16 <- function(source, format = NULL, na = NULL) {
  new_column(source, "i16", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_i32 <- function(source, format = NULL, na = NULL) {
  new_column(source, "i32", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_i64 <- function(source, format = NULL, na = NULL) {
  new_column(source, "i64", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_u8 <- function(source, format = NULL, na = NULL) {
  new_column(source, "u8", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_u16 <- function(source, format = NULL, na = NULL) {
  new_column(source, "u16", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_u32 <- function(source, format = NULL, na = NULL) {
  new_column(source, "u32", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_u64 <- function(source, format = NULL, na = NULL) {
  new_column(source, "u64", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_f32 <- function(source, format = NULL, na = NULL) {
  new_column(source, "f32", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_f64 <- function(source, format = NULL, na = NULL) {
  new_column(source, "f64", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_decimal <- function(source, format = NULL, na = NULL) {
  new_column(source, "decimal", format = format, na = na)
}

#' @rdname col_bool
#' @export
col_uuid <- function(source, na = NULL) new_column(source, "uuid", na = na)

#' @rdname col_bool
#' @export
col_timestamp <- function(source, options = NULL, na = NULL) {
  new_column(source, "timestamp", na = na, options = options)
}

#' @rdname col_bool
#' @export
col_unix <- function(source, precision, na = NULL) {
  new_column(source, "unix", declared(precision, precisions, "precision"), na = na)
}

#' @rdname col_bool
#' @export
col_excel_serial <- function(source, epoch, na = NULL) {
  new_column(source, "excel_serial", declared(epoch, epochs, "epoch"), na = na)
}

#' @rdname col_bool
#' @export
col_date <- function(source, order = NULL, options = NULL, months = NULL, na = NULL) {
  if (is.null(order)) {
    return(new_column(source, "date", na = na, options = options, months = months))
  }
  new_column(source, "date_ordered", declared(order, orders, "order"), factory = "date",
             na = na, options = options, months = months)
}

#' @rdname col_bool
#' @export
col_datetime <- function(source, order, options = NULL, months = NULL, na = NULL) {
  new_column(source, "datetime", declared(order, orders, "order"), na = na, options = options,
             months = months)
}

#' @rdname col_bool
#' @export
col_time <- function(source, na = NULL) new_column(source, "time", na = na)

#' @rdname col_bool
#' @export
col_duration <- function(source, na = NULL) new_column(source, "duration", na = na)

#' @rdname col_bool
#' @export
col_text <- function(source, na = NULL, raw = FALSE) {
  new_column(source, "text", na = na, raw = raw)
}

# A column as the call that makes it, pasteable: `format` the text its num_format prints as.
column_call <- function(x, format = NULL) {
  source <- if (is.character(x$source)) encodeString(x$source, quote = "\"") else x$source
  extra <- switch(x$door,
    unix = paste0(", \"", precisions[[x$param]], "\""),
    excel_serial = paste0(", \"", epochs[[x$param]], "\""),
    date_ordered = , datetime = paste0(", \"", orders[[x$param]], "\""),
    ""
  )
  if (!is.null(x$num_format)) {
    extra <- paste0(", ", format %||%
                      paste0("hypercast::", trimws(utils::capture.output(print(x$num_format)))))
  }
  if (!is.null(x$options)) {
    extra <- paste0(extra, ", options = hypercast::",
                    trimws(utils::capture.output(print(x$options))))
  }
  if (!is.null(x$months)) extra <- paste0(extra, ", months = <month_names>")
  if (!is.null(x$na)) {
    extra <- paste0(extra, ", na = ", if (length(x$na)) {
      paste0("c(", paste(encodeString(x$na, quote = "\""), collapse = ", "), ")")
    } else {
      "character(0)"
    })
  }
  if (isTRUE(x$raw)) extra <- paste0(extra, ", raw = TRUE")
  sprintf("col_%s(%s%s)", x$factory, source, extra)
}

#' @rdname col_bool
#' @param x A `hypertabular_column`.
#' @param ... Ignored.
#' @export
print.hypertabular_column <- function(x, ...) {
  cat(column_call(x), "\n", sep = "")
  invisible(x)
}

# A plan as a list of columns, each checked.
as_plan <- function(plan) {
  if (inherits(plan, "hypertabular_column")) plan <- list(plan)
  if (!is.list(plan) || is.object(plan)) {
    stop("plan must be a list of columns, as col_i32() and its siblings make", call. = FALSE)
  }
  for (i in seq_along(plan)) {
    if (!inherits(plan[[i]], "hypertabular_column")) {
      stop(sprintf("plan column %d is not a column; col_i32() and its siblings make them", i),
           call. = FALSE)
    }
  }
  plan
}

# "1 and 2", "1, 2 and 3".
and_list <- function(x) {
  if (length(x) < 2L) return(as.character(x))
  paste(paste(x[-length(x)], collapse = ", "), "and", x[[length(x)]])
}

# The plan's zero-based source ordinals against `header` (NULL when there is none), and the
# name each output column takes: the plan's own, else the header's, else V and the position.
# A source named by a header that holds the name more than once is an error naming each place.
resolve_plan <- function(plan, header) {
  ordinals <- integer(length(plan))
  out <- character(length(plan))
  given <- names(plan)
  for (i in seq_along(plan)) {
    source <- plan[[i]]$source
    if (is.character(source)) {
      if (is.null(header)) {
        stop(sprintf("plan column %d names its source \"%s\", and there is no header to find it in",
                     i, source), call. = FALSE)
      }
      at <- which(header == source)
      if (!length(at)) {
        stop(sprintf("plan column %d names its source \"%s\", which is not in the header", i,
                     source), call. = FALSE)
      }
      if (length(at) > 1L) {
        stop(sprintf(paste("plan column %d names its source \"%s\", which the header holds at",
                           "columns %s: name it by position"), i, source, and_list(at)),
             call. = FALSE)
      }
    } else {
      at <- source
    }
    ordinals[[i]] <- at - 1L
    name <- if (!is.null(given) && !is.na(given[[i]]) && nzchar(given[[i]])) given[[i]]
    if (is.null(name)) {
      name <- if (!is.null(header) && at <= length(header) && !is.na(header[[at]]) &&
                    nzchar(header[[at]])) header[[at]] else paste0("V", at)
    }
    out[[i]] <- name
  }
  list(ordinals = ordinals, names = out)
}

# Output names as a data frame needs them, unique: a repeated name gains _1, _2, ...
repair_names <- function(names) make.unique(names, sep = "_")

# A count a u32 field holds, as its four little-endian bytes. Doubles, so 2^31 and past fit.
u32_le <- function(value) {
  value <- as.numeric(value)
  as.raw(c(value %% 256, value %/% 256 %% 256, value %/% 65536 %% 256,
           value %/% 16777216 %% 256))
}

# The plan as the C shim takes it: each column's ordinal, door, param, format and date options
# (116 bytes), each column's month vocabulary and tokens, and which text columns come back raw.
pack_specs <- function(plan, ordinals) {
  specs <- unlist(lapply(seq_along(plan), function(i) {
    column <- plan[[i]]
    c(u32_le(ordinals[[i]]), u32_le(door_codes[[column$door]]), u32_le(column$param),
      column$format, packed_date(column$options))
  }), use.names = FALSE) %||% raw()
  lists <- lapply(plan, function(column) {
    list(if (!is.null(column$months)) column$months$packed, packed_tokens(column$na))
  })
  raws <- vapply(plan, function(column) isTRUE(column$raw), FALSE)
  list(specs = specs, lists = lists, raws = raws)
}

# What a plan column declares, judged by the core before anything is read: an R error naming
# the first column it refuses, and both packages' versions where a layout's size is the cause.
judge_plan <- function(plan, packed) {
  refused <- .Call(C_judge, packed$specs, packed$lists)
  if (refused == 0L) {
    return(invisible())
  }
  column <- plan[[refused]]
  layout <- .Call(C_layout)
  sizes <- c(if (column$door %in% numeric_doors && !is.null(column$num_format)) {
    size_word(column$num_format$packed)
  }, if (!is.null(column$options)) size_word(column$options$packed))
  unread <- (column$door %in% numeric_doors && !is.null(column$num_format) &&
               !size_in(size_word(column$num_format$packed), layout[[2]], layout[[1]])) ||
    (!is.null(column$options) && !size_in(size_word(column$options$packed), layout[[4]],
                                          layout[[3]]))
  if (unread) {
    stop(sprintf(paste("plan column %d cannot be honoured: hypercast %s packs a layout of %s",
                       "bytes, and hypertabular %s reads a number format of %d to %d bytes and",
                       "date options of %d to %d; install versions of the two that agree"),
                 refused, utils::packageVersion("hypercast"), paste(sizes, collapse = " and "),
                 utils::packageVersion("hypertabular"), layout[[2]], layout[[1]], layout[[4]],
                 layout[[3]]), call. = FALSE)
  }
  stop(sprintf("plan column %d cannot be honoured: the core refuses what it declares", refused),
       call. = FALSE)
}

size_in <- function(size, least, most) !is.na(size) && size >= least && size <= most

`%||%` <- function(a, b) if (is.null(a)) b else a
