#' Workbooks
#'
#' `workbook()` opens an XLSX, ODS or Numbers workbook held in memory (the whole file is read
#' in), lists its sheets and loads its shared strings and styles: what [sheet_reader()] and
#' [read_sheet()] read from. Which kind of workbook it is is read from its bytes, never from
#' a name. Its sheets can be read at once, each with a reader of its own.
#'
#' A workbook that cannot be read (not a zip, encrypted, a part missing or broken) is not an
#' R error: `workbook()` returns it with its `failure` set (a [hypertabular_failure]) and no
#' sheets, and [read_sheet()] returns its failure as the table's.
#'
#' @param source The workbook: a file path or a raw vector.
#' @return A `hypertabular_workbook`: a list of `format` (`"xlsx"`, `"ods"` or `"numbers"`),
#'   `date_system` (`"y1900"` or `"y1904"`, the system the workbook's serials count in, as
#'   HyperCast names it), `sheets` (a data frame of each sheet's `name` and whether it is
#'   `hidden`, in the workbook's own order; chart and macro sheets are not among them) and
#'   `failure` (`NULL` for a workbook that opened). `NA` for the first two of one that did not.
#' @family reads
#' @export
#' @examples
#' path <- system.file("extdata", "basic.xlsx", package = "hypertabular")
#' book <- workbook(path)
#' book
#' read_sheet(book, "Data", list(id = col_i32("id"), amount = col_decimal("amount")))
#' workbook(charToRaw("not a workbook"))$failure
workbook <- function(source) {
  if (is.character(source) && length(source) == 1L && !is.na(source)) {
    size <- file.size(source)
    if (is.na(size)) stop(sprintf("cannot open file '%s'", source), call. = FALSE)
    source <- readBin(source, "raw", size)
  } else if (!is.raw(source)) {
    stop("source must be a file path or a raw vector", call. = FALSE)
  }
  res <- .Call(C_workbook_open, source, stingy())
  sheets <- data.frame(name = character(), hidden = logical())
  if (res$status == 3L) {
    return(structure(list(format = NA_character_, date_system = NA_character_,
                          sheets = sheets, failure = new_failure(res$failure)),
                     class = "hypertabular_workbook"))
  }
  p <- res$payload
  structure(
    list(format = c("xlsx", "ods", "numbers")[[p[[2]]]],
         date_system = epochs[[p[[3]]]],
         sheets = data.frame(name = p[[4]], hidden = p[[5]]),
         failure = NULL, ptr = p[[1]], parts = p[[6]], indexes = p[[7]]),
    class = "hypertabular_workbook"
  )
}

#' @export
print.hypertabular_workbook <- function(x, ...) {
  if (!is.null(x$failure)) {
    cat("<hypertabular_workbook> failed:", format(x$failure), "\n")
  } else {
    cat(sprintf("<hypertabular_workbook: %s, %s> %d sheet%s\n", x$format, x$date_system,
                nrow(x$sheets), if (nrow(x$sheets) == 1L) "" else "s"))
    print(x$sheets)
  }
  invisible(x)
}

check_workbook <- function(book) {
  if (!inherits(book, "hypertabular_workbook")) {
    stop("book must be a hypertabular_workbook, as workbook() returns", call. = FALSE)
  }
  if (!is.null(book$failure)) {
    stop("the workbook could not be read (", format(book$failure), "); see its $failure",
         call. = FALSE)
  }
}

sheet_index <- function(book, sheet) {
  count <- nrow(book$sheets)
  if (is.character(sheet) && length(sheet) == 1L && !is.na(sheet)) {
    at <- match(sheet, book$sheets$name)
    if (is.na(at)) {
      stop(sprintf("the workbook has no sheet named \"%s\"", sheet), call. = FALSE)
    }
    return(at)
  }
  if (is.numeric(sheet) && length(sheet) == 1L && !is.na(sheet) && sheet == floor(sheet)) {
    if (sheet < 1 || sheet > count) {
      stop(sprintf("the workbook has %d sheet%s, and no sheet %s", count,
                   if (count == 1L) "" else "s", format(sheet)), call. = FALSE)
    }
    return(as.integer(sheet))
  }
  stop("sheet must be one sheet name, or one whole-number position from 1", call. = FALSE)
}

#' Read a sheet
#'
#' `sheet_reader()` starts a forward-only read of one sheet of a [workbook()], a batch at a
#' time, through the same [read_batch()] delimited text is read with. With a header declared,
#' the sheet's first row is read as it opens ([header()]): its names are each cell said as
#' text, and the plan, given here or later through [bind()], may name source columns by them.
#' `read_sheet()` reads the whole sheet through a plan and returns it as one
#' `hypertabular_table`.
#'
#' A typed cell is cast directly by its door (a stored `42` never passes through text to
#' become an integer), and a text cell goes through the door as delimited text would. A sheet
#' that is structurally broken ends in a [hypertabular_failure], a value, after every intact
#' row before it has been delivered: no read signals an R error for anything in its data.
#'
#' @param book A `hypertabular_workbook`. For `read_sheet()` also a file path or a raw
#'   vector, which it opens.
#' @param sheet The sheet: its name (the first of equal names) or its position in
#'   `book$sheets`, counted from 1. A hidden sheet reads like any other.
#' @param plan A list of columns ([col_bool()] and its siblings), named for the output
#'   columns; or `NULL`, to bind one later. For `read_sheet()`, `NULL` reads every header
#'   column through [col_text()].
#' @param has_header Whether the sheet's first row is a header, never delivered as a row.
#' @param skip_empty_rows Whether a row with no cells is skipped (row-number gaps and empty
#'   rows alike).
#' @param batch_rows The most rows a batch holds.
#' @return `sheet_reader()` returns a `hypertabular_sheet_reader`; `read_sheet()` a
#'   `hypertabular_table`.
#' @family reads
#' @export
#' @examples
#' path <- system.file("extdata", "basic.xlsx", package = "hypertabular")
#' reader <- sheet_reader(workbook(path), "Data")
#' header(reader)
#' bind(reader, list(col_text("name"), col_date("day")))
#' read_batch(reader)$columns
sheet_reader <- function(book, sheet = 1L, plan = NULL, has_header = TRUE,
                         skip_empty_rows = TRUE, batch_rows = 4096L) {
  check_workbook(book)
  index <- sheet_index(book, sheet)
  flag <- function(value, what) {
    if (!is.logical(value) || length(value) != 1L || is.na(value)) {
      stop(sprintf("%s must be TRUE or FALSE", what), call. = FALSE)
    }
    value
  }
  has_header <- flag(has_header, "has_header")
  skip_empty_rows <- flag(skip_empty_rows, "skip_empty_rows")
  batch_rows <- count_arg(batch_rows, "batch_rows")
  if (!is.null(plan)) plan <- as_plan(plan)
  reader <- new.env(parent = emptyenv())
  reader$batch_rows <- batch_rows
  reader$header <- NULL
  reader$failure <- NULL
  reader$pending <- NULL
  reader$done <- FALSE
  reader$plan <- NULL
  reader$names <- character()
  class(reader) <- c("hypertabular_sheet_reader", "hypertabular_reader")
  res <- .Call(C_sheet_new, book$ptr, book$parts[[index]], book$indexes[[index]], has_header,
               skip_empty_rows, stingy())
  reader$ptr <- res$payload
  if (res$status == 3L) {
    end_read(reader, new_failure(res$failure))
  } else if (has_header) {
    res <- .Call(C_sheet_header, reader$ptr)
    if (res$status == 3L) {
      end_read(reader, new_failure(res$failure))
    } else {
      reader$header <- res$payload
    }
  }
  if (!is.null(plan)) bind(reader, plan)
  reader
}

bind_sheet <- function(reader, plan) {
  resolved <- resolve_plan(plan, reader$header)
  width <- max(c(resolved$ordinals, -1L)) + 1L
  .Call(C_sheet_bind, reader$ptr, pack_plan(plan, resolved$ordinals), reader$batch_rows, width)
  reader$plan <- plan
  reader$names <- resolved$names
}

read_sheet_batch <- function(reader) {
  res <- .Call(C_sheet_fill, reader$ptr)
  if (!is.null(res$failure)) {
    if (res$status == 0L) {
      reader$pending <- new_failure(res$failure)
    } else {
      end_read(reader, new_failure(res$failure))
    }
  } else if (res$status == 2L) {
    end_read(reader)
  }
  res$payload
}

#' @rdname sheet_reader
#' @export
read_sheet <- function(book, sheet = 1L, plan = NULL, has_header = TRUE, skip_empty_rows = TRUE,
                       batch_rows = 4096L) {
  if (!inherits(book, "hypertabular_workbook")) book <- workbook(book)
  if (!is.null(book$failure)) {
    if (!is.null(plan)) plan <- as_plan(plan)
    reader <- list(header = NULL, failure = book$failure)
    return(new_table(reader, list(), plan %||% list(), resolve_names(plan %||% list(), NULL)))
  }
  reader <- sheet_reader(book, sheet, has_header = has_header, skip_empty_rows = skip_empty_rows,
                         batch_rows = batch_rows)
  read_all(reader, plan)
}
