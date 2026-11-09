#' Workbooks
#'
#' `workbook()` opens an XLSX, ODS or Numbers workbook held in memory (the whole file is read
#' in), lists its sheets and loads its shared strings and styles: what [sheet_reader()] and
#' [read_sheet()] read from. Which kind of workbook it is is read from its bytes, never from
#' a name. Its sheets can be read at once, each with a reader of its own.
#'
#' `close()` lets go of the workbook's native memory now rather than at the next garbage
#' collection: no sheet can be opened from it after, and the sheets already open read on, each
#' holding what it needs until it is closed itself. It is safe to call again.
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
#' close(book)
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
  state <- new.env(parent = emptyenv())
  state$closed <- FALSE
  if (res$status == 3L) {
    return(structure(list(format = NA_character_, date_system = NA_character_,
                          sheets = sheets, failure = new_failure(res$failure), state = state),
                     class = "hypertabular_workbook"))
  }
  p <- res$payload
  state$ptr <- p[[1]]
  structure(
    list(format = c("xlsx", "ods", "numbers")[[p[[2]]]],
         date_system = epochs[[p[[3]]]],
         sheets = data.frame(name = p[[4]], hidden = p[[5]]),
         failure = NULL, state = state, parts = p[[6]], indexes = p[[7]]),
    class = "hypertabular_workbook"
  )
}

#' @rdname workbook
#' @param con A `hypertabular_workbook`.
#' @param ... Ignored.
#' @export
close.hypertabular_workbook <- function(con, ...) {
  state <- con$state
  if (!is.null(state) && !isTRUE(state$closed)) {
    if (!is.null(state$ptr)) .Call(C_workbook_close, state$ptr)
    state$ptr <- NULL
    state$closed <- TRUE
  }
  invisible(con)
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
  if (isTRUE(book$state$closed)) {
    stop("the workbook is closed: no sheet can be opened from it after close()", call. = FALSE)
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
#' the sheet's first row read is a header ([header()]): its names are each cell said as text,
#' and the plan, given here or later through [bind()], may name source columns by them.
#' `read_sheet()` reads the whole sheet through a plan and returns it as one
#' `hypertabular_table`. [close()][close.hypertabular_reader] lets go of a reader early.
#'
#' A typed cell is cast directly by its door (a stored `42` never passes through text to
#' become an integer), and a text cell goes through the door as delimited text would. A sheet
#' that is structurally broken ends in a [hypertabular_failure], a value, after every intact
#' row before it has been delivered: no read signals an R error for anything in its data.
#'
#' What is read is in a fixed order: the `range` selects rows and columns, `skip` passes over
#' rows inside it, `header_row` discards rows after those, then comes the header, and
#' `skip_empty_rows` holds throughout. A plan's position 1 is the range's first column, and
#' [problems()] says each cell's column of the sheet, `A` being 1, wherever the range starts.
#'
#' @param book A `hypertabular_workbook`. For `read_sheet()` also a file path or a raw
#'   vector, which it opens.
#' @param sheet The sheet: its name (the first of equal names) or its position in
#'   `book$sheets`, counted from 1. A hidden sheet reads like any other.
#' @param plan A list of columns ([col_bool()] and its siblings), named for the output
#'   columns; or `NULL`, to bind one later. For `read_sheet()`, `NULL` reads every header
#'   column through [col_text()].
#' @param has_header Whether the sheet's first row read is a header, never delivered as a row.
#' @param skip_empty_rows Whether a row with no cells is skipped (row-number gaps and empty
#'   rows alike). Such a row still counts toward `skip`, and doesn't toward `header_row`.
#' @param batch_rows The most rows a batch holds.
#' @param skip Rows of the sheet to pass over at the start of the range, empty ones included;
#'   their cells aren't judged.
#' @param header_row Rows to read and discard after those, as the read delivers them.
#' @param na The cells every column reads as missing, unless it declares its own: an exact
#'   match of a text cell's bytes is an `empty` verdict at origin `"token"`. `character(0)`
#'   declares none.
#' @param range The cells to read, such as `"A5:F200"`, `"B3:D"` or `"C:F"`, as the core reads
#'   a range; `NULL` reads the whole sheet. No row past its last is read, so a failure in one
#'   isn't met.
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
#' close(reader)
sheet_reader <- function(book, sheet = 1L, plan = NULL, has_header = TRUE,
                         skip_empty_rows = TRUE, batch_rows = 4096, skip = 0, header_row = 0,
                         na = character(), range = NULL) {
  check_workbook(book)
  index <- sheet_index(book, sheet)
  has_header <- flag_arg(has_header, "has_header")
  skip_empty_rows <- flag_arg(skip_empty_rows, "skip_empty_rows")
  batch_rows <- count_arg(batch_rows, "batch_rows")
  skip <- u32_arg(skip, "skip")
  header_row <- u32_arg(header_row, "header_row")
  na <- check_tokens(na, FALSE)
  corners <- range_of(range)
  check_sizes(NULL, batch_rows, 1, batch_notes + batch_sheet)
  if (!is.null(plan)) {
    plan <- as_plan(plan)
    judge_plan(plan, pack_specs(plan, integer(length(plan))))
  }
  reader <- new_reader(c("hypertabular_sheet_reader", "hypertabular_reader"), batch_rows)
  reader$flags <- batch_notes + batch_sheet
  reader$sheet <- book$sheets$name[[index]]
  reader$first_column <- max(corners[[2]], 1)
  reader$book <- book
  reader$index <- index
  reader$options <- c(u32_le(36), u32_le(has_header), u32_le(skip_empty_rows), u32_le(skip),
                      u32_le(header_row), u32_le(corners[[1]]), u32_le(corners[[2]]),
                      u32_le(corners[[3]]), u32_le(corners[[4]]))
  reader$tokens <- if (length(na)) packed_tokens(na)
  res <- .Call(C_sheet_new, book$state$ptr, book$parts[[index]], book$indexes[[index]],
               reader$options, reader$tokens, stingy())
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

# A range's four corners as the core reads them, each 0 for no limit: c(0, 0, 0, 0) for NULL.
range_of <- function(range) {
  if (is.null(range)) {
    return(c(0, 0, 0, 0))
  }
  corners <- if (is.character(range) && length(range) == 1L && !is.na(range)) {
    .Call(C_range_parse, charToRaw(enc2utf8(range)))
  }
  if (is.null(corners)) {
    stop("range must be one range such as \"A5:F200\", or NULL", call. = FALSE)
  }
  corners
}

bind_sheet <- function(reader, plan) {
  resolved <- resolve_plan(plan, reader$header)
  packed <- pack_specs(plan, resolved$ordinals)
  judge_plan(plan, packed)
  check_sizes(packed, reader$batch_rows, 1, reader$flags)
  width <- max(c(resolved$ordinals, -1L)) + 1L
  .Call(C_sheet_bind, reader$ptr, packed$specs, packed$lists, packed$raws, reader$batch_rows,
        width, isTRUE(reader$widths))
  resolved
}

read_sheet_batch <- function(reader, sample = FALSE) {
  res <- interruptible(reader, .Call(C_sheet_fill, reader$ptr, sample))
  if (!is.null(res$failure) && res$status != 0L) {
    end_read(reader, new_failure(res$failure))
  } else if (res$status == 2L) {
    end_read(reader)
  }
  if (is.null(res$payload)) {
    return(NULL)
  }
  # Rows that came with a failure: the read ends on it once they're R's (received()).
  structure(res$payload, first = attr(res, "first"),
            pending = if (!is.null(res$failure)) new_failure(res$failure))
}

#' @rdname sheet_reader
#' @export
read_sheet <- function(book, sheet = 1L, plan = NULL, has_header = TRUE, skip_empty_rows = TRUE,
                       batch_rows = 4096, skip = 0, header_row = 0, na = character(),
                       range = NULL) {
  if (!inherits(book, "hypertabular_workbook")) {
    book <- workbook(book)
    on.exit(close(book), add = TRUE)
  }
  if (!is.null(book$failure)) {
    if (!is.null(plan)) plan <- as_plan(plan)
    reader <- list(header = NULL, failure = book$failure)
    plan <- plan %||% list()
    return(new_table(reader, list(), plan, resolve_names(plan)))
  }
  reader <- sheet_reader(book, sheet, has_header = has_header, skip_empty_rows = skip_empty_rows,
                         batch_rows = batch_rows, skip = skip, header_row = header_row, na = na,
                         range = range)
  on.exit(close(reader), add = TRUE)
  read_all(reader, plan)
}
