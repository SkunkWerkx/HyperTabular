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
# The widest source position a plan may name: the core's 2^20 columns.
max_position <- 2^20

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
  if (is.numeric(source)) as.integer(source) else source
}

# The 92 bytes of HyperCast's RawNumFormat the core reads a numeric column with: the
# format's own (num_format()'s `packed`), or for none the invariant notation's: hypercast's,
# with its strict grouping (all zeros would be the C ABI's legacy reading).
packed_format <- function(format) {
  if (is.null(format)) {
    return(hypercast::num_format()$packed)
  }
  if (!inherits(format, "hypercast_num_format") || !is.raw(format$packed) ||
        length(format$packed) != 92L) {
    stop("format must be a hypercast_num_format, as hypercast::num_format() returns, or NULL",
         call. = FALSE)
  }
  format$packed
}

new_column <- function(source, door, param = 0L, format = NULL, factory = door) {
  structure(
    list(source = check_source(source), door = door, param = param,
         format = if (door %in% numeric_doors) packed_format(format) else raw(92L),
         num_format = format, factory = factory),
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
#' (`NULL`, the default, is the invariant notation); `col_uuid()`; `col_timestamp()`,
#' `col_unix()` and `col_excel_serial()` the instants; `col_date()`, `col_datetime()`,
#' `col_time()` and `col_duration()` the civil values. `col_text()` asks for the cell's text
#' itself, cast by nothing: its value is the text, and only an empty cell is a fault.
#'
#' A workbook's typed cells (numbers, dates, booleans, errors) go through the door as the
#' cast matrix in the project's `docs/design.md` says: a stored number through the door's
#' typed twin, never through text, and a typed cell through `col_text()` as its canonical
#' text.
#'
#' @param source The source column: its name in the header (matched exactly, the first of
#'   equal names) or its position, counted from 1. A position past the end of a row reads as
#'   an empty cell.
#' @param format A [hypercast::num_format()], or `NULL` for the invariant notation.
#' @param precision The Unix unit: `"seconds"`, `"milliseconds"`, `"microseconds"` or
#'   `"nanoseconds"`.
#' @param epoch The date system: `"y1900"` or `"y1904"`. A workbook says which it uses
#'   (`workbook()$date_system`); this door reads cells as the plan declares.
#' @param order The field order: `"year_month_day"`, `"month_day_year"` or
#'   `"day_month_year"`. `NULL`, for `col_date()`, reads strict ISO only.
#' @return A `hypertabular_column`.
#' @family plans
#' @export
#' @examples
#' plan <- list(
#'   id = col_i32("id"),
#'   amount = col_decimal("amount", hypercast::num_format(",", ".")),
#'   when = col_date(3, order = "day_month_year"),
#'   note = col_text("note")
#' )
#' plan$id
col_bool <- function(source) new_column(source, "bool")

#' @rdname col_bool
#' @export
col_i8 <- function(source, format = NULL) new_column(source, "i8", format = format)

#' @rdname col_bool
#' @export
col_i16 <- function(source, format = NULL) new_column(source, "i16", format = format)

#' @rdname col_bool
#' @export
col_i32 <- function(source, format = NULL) new_column(source, "i32", format = format)

#' @rdname col_bool
#' @export
col_i64 <- function(source, format = NULL) new_column(source, "i64", format = format)

#' @rdname col_bool
#' @export
col_u8 <- function(source, format = NULL) new_column(source, "u8", format = format)

#' @rdname col_bool
#' @export
col_u16 <- function(source, format = NULL) new_column(source, "u16", format = format)

#' @rdname col_bool
#' @export
col_u32 <- function(source, format = NULL) new_column(source, "u32", format = format)

#' @rdname col_bool
#' @export
col_u64 <- function(source, format = NULL) new_column(source, "u64", format = format)

#' @rdname col_bool
#' @export
col_f32 <- function(source, format = NULL) new_column(source, "f32", format = format)

#' @rdname col_bool
#' @export
col_f64 <- function(source, format = NULL) new_column(source, "f64", format = format)

#' @rdname col_bool
#' @export
col_decimal <- function(source, format = NULL) new_column(source, "decimal", format = format)

#' @rdname col_bool
#' @export
col_uuid <- function(source) new_column(source, "uuid")

#' @rdname col_bool
#' @export
col_timestamp <- function(source) new_column(source, "timestamp")

#' @rdname col_bool
#' @export
col_unix <- function(source, precision) {
  new_column(source, "unix", declared(precision, precisions, "precision"))
}

#' @rdname col_bool
#' @export
col_excel_serial <- function(source, epoch) {
  new_column(source, "excel_serial", declared(epoch, epochs, "epoch"))
}

#' @rdname col_bool
#' @export
col_date <- function(source, order = NULL) {
  if (is.null(order)) {
    return(new_column(source, "date"))
  }
  new_column(source, "date_ordered", declared(order, orders, "order"), factory = "date")
}

#' @rdname col_bool
#' @export
col_datetime <- function(source, order) {
  new_column(source, "datetime", declared(order, orders, "order"))
}

#' @rdname col_bool
#' @export
col_time <- function(source) new_column(source, "time")

#' @rdname col_bool
#' @export
col_duration <- function(source) new_column(source, "duration")

#' @rdname col_bool
#' @export
col_text <- function(source) new_column(source, "text")

#' @rdname col_bool
#' @param x A `hypertabular_column`.
#' @param ... Ignored.
#' @export
print.hypertabular_column <- function(x, ...) {
  source <- if (is.character(x$source)) encodeString(x$source, quote = "\"") else x$source
  extra <- switch(x$door,
    unix = paste0(", \"", precisions[[x$param]], "\""),
    excel_serial = paste0(", \"", epochs[[x$param]], "\""),
    date_ordered = , datetime = paste0(", \"", orders[[x$param]], "\""),
    ""
  )
  if (!is.null(x$num_format)) extra <- ", <num_format>"
  cat(sprintf("col_%s(%s%s)\n", x$factory, source, extra))
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

# The plan's zero-based source ordinals against `header` (NULL when there is none), and the
# name each output column takes.
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
      at <- match(source, header)
      if (is.na(at)) {
        stop(sprintf("plan column %d names its source \"%s\", which is not in the header", i,
                     source), call. = FALSE)
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

u32_le <- function(value) writeBin(as.integer(value), raw(), size = 4L, endian = "little")

# Every plan column as the core's 104-byte ColumnSpec, little-endian.
pack_plan <- function(plan, ordinals) {
  unlist(lapply(seq_along(plan), function(i) {
    column <- plan[[i]]
    c(u32_le(ordinals[[i]]), u32_le(door_codes[[column$door]]), u32_le(column$param),
      column$format)
  }), use.names = FALSE) %||% raw()
}

`%||%` <- function(a, b) if (is.null(a)) b else a
