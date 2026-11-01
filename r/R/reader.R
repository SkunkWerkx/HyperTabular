check_reader <- function(x) {
  if (!inherits(x, "hypertabular_reader")) {
    stop("x must be a reader, as delimited_reader() or sheet_reader() returns", call. = FALSE)
  }
}

#' Reading a batch at a time
#'
#' A reader ([delimited_reader()], [sheet_reader()]) is opened without a plan or with one;
#' `header()` is the header it read as it opened, and `bind()` gives it its plan once, so a
#' plan can name source columns by the header's names. `read_batch()` reads the next batch
#' of up to the reader's `batch_rows` rows, as a [hypertabular_batch], and returns `NULL` once
#' there are no more: at the end of the input, or at a structural failure, which
#' [failure()] then holds. A batch is a copy, and stays good after the reader moves on.
#'
#' @param x A reader.
#' @param plan A list of columns ([col_bool()] and its siblings), named for the output
#'   columns.
#' @return `header()` returns the header's names as a character vector (a name holding a NUL
#'   byte is `NA`), `character(0)` for an input with no record, or `NULL` when no header is
#'   declared. `bind()` returns `x` invisibly. `read_batch()` returns a `hypertabular_batch`
#'   or `NULL`.
#' @family reads
#' @export
#' @examples
#' reader <- delimited_reader(charToRaw("a,b\n1,x\n2,y\n3,z\n"), batch_rows = 2)
#' header(reader)
#' bind(reader, list(col_i32("a"), col_text("b")))
#' while (!is.null(batch <- read_batch(reader))) print(batch$columns$a$value)
#' failure(reader)
header <- function(x) {
  check_reader(x)
  x$header
}

#' @rdname header
#' @export
bind <- function(x, plan) {
  check_reader(x)
  if (!is.null(x$plan)) {
    stop("the reader already has a plan: a plan is bound once", call. = FALSE)
  }
  plan <- as_plan(plan)
  if (inherits(x, "hypertabular_delimited_reader")) {
    bind_delimited(x, plan)
  } else {
    bind_sheet(x, plan)
  }
  invisible(x)
}

#' @rdname header
#' @export
read_batch <- function(x) {
  raw <- read_raw_batch(x)
  if (is.null(raw)) NULL else new_batch(raw, x$plan, x$names)
}

# The next batch as the C shim wrote it, unclassed, or NULL.
read_raw_batch <- function(x) {
  check_reader(x)
  if (is.null(x$plan)) {
    stop("the reader has no plan yet: bind() one before reading", call. = FALSE)
  }
  if (!is.null(x$pending)) {
    end_read(x, x$pending)
    x$pending <- NULL
  }
  if (isTRUE(x$done)) {
    return(NULL)
  }
  if (inherits(x, "hypertabular_delimited_reader")) {
    read_delimited_batch(x)
  } else {
    read_sheet_batch(x)
  }
}

# The whole read through `plan` (every header column as text when NULL), as a table.
read_all <- function(reader, plan) {
  if (is.null(plan)) {
    if (is.null(reader$header)) {
      if (!is.null(reader$failure)) {
        return(new_table(reader, list(), list(), character()))
      }
      stop("plan must be given when there is no header to read every column of", call. = FALSE)
    }
    plan <- lapply(seq_along(reader$header), col_text)
  }
  plan <- as_plan(plan)
  if (!is.null(reader$failure)) {
    resolved <- resolve_names(plan, reader$header)
    return(new_table(reader, list(), plan, resolved))
  }
  bind(reader, plan)
  raws <- list()
  while (!is.null(raw <- read_raw_batch(reader))) {
    raws[[length(raws) + 1L]] <- raw
  }
  new_table(reader, raws, plan, reader$names)
}

# The output names of a plan no read will bind, as resolve_plan() would name them.
resolve_names <- function(plan, header) {
  vapply(seq_along(plan), function(i) {
    given <- names(plan)
    if (!is.null(given) && nzchar(given[[i]])) return(given[[i]])
    source <- plan[[i]]$source
    if (is.character(source)) source else paste0("V", source)
  }, "")
}

new_table <- function(reader, raws, plan, names) {
  bound <- bind_batches(raws, plan, names)
  structure(
    list(header = reader$header, columns = bound$columns, line = bound$line,
         failure = reader$failure),
    class = c("hypertabular_table", "hypertabular_batch")
  )
}

#' @export
print.hypertabular_reader <- function(x, ...) {
  kind <- if (inherits(x, "hypertabular_delimited_reader")) "delimited" else "sheet"
  state <- if (!is.null(x$failure)) {
    paste("failed:", format(x$failure))
  } else if (isTRUE(x$done)) {
    "at the end"
  } else if (is.null(x$plan)) {
    "unbound"
  } else {
    sprintf("%d plan columns", length(x$plan))
  }
  cat(sprintf("<hypertabular %s reader> %s\n", kind, state))
  invisible(x)
}

#' The version of the native core
#'
#' The version of the Rust core this session actually loaded, decoded from the packed
#' `hypertabular_version` export every binding probes. It equals the package version.
#'
#' @return A [package_version] of length 1.
#' @export
#' @examples
#' hypertabular_native_version()
hypertabular_native_version <- function() {
  v <- .Call(C_native_version)
  package_version(sprintf(
    "%d.%d.%d",
    bitwAnd(bitwShiftR(v, 16L), 255L),
    bitwAnd(bitwShiftR(v, 8L), 255L),
    bitwAnd(v, 255L)
  ))
}
