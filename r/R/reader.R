check_reader <- function(x) {
  if (!inherits(x, "hypertabular_reader")) {
    stop("x must be a reader, as delimited_reader() or sheet_reader() returns", call. = FALSE)
  }
}

new_reader <- function(class, batch_rows) {
  reader <- new.env(parent = emptyenv())
  reader$batch_rows <- batch_rows
  reader$con <- NULL
  reader$own <- FALSE
  reader$ptr <- NULL
  reader$header <- NULL
  reader$failure <- NULL
  reader$pending <- NULL
  reader$done <- FALSE
  reader$closed <- FALSE
  # The frame a call into the reader runs in, or NULL: see enter().
  reader$busy <- NULL
  reader$interruptible <- FALSE
  reader$handing <- FALSE
  reader$plan <- NULL
  reader$names <- character()
  reader$ordinals <- integer()
  reader$sheet <- NA_character_
  reader$first_column <- 1
  reader$long_rows <- 0L
  reader$widths <- FALSE
  class(reader) <- class
  reader
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

# Marks the reader busy for the caller's frame, or refuses: a reader is called into once at a
# time, so a handler, a finalizer or a connection's own R code that reaches it during a read, a
# bind, a close or a sample is an R error and leaves the read as it was. The mark is the frame
# itself, undone when it exits; a mark whose frame is gone (an interrupt during that undoing) is
# stale, and doesn't hold the reader. `interruptible` says whether the steps an interrupt can't
# leave half done run with interrupts allowed (see interruptible()).
enter <- function(x, what, frame = parent.frame(), interruptible = FALSE) {
  # The caller's frame, taken here: forced later, inside bquote(), it would be bquote's own.
  force(frame)
  check_reader(x)
  owner <- x$busy
  if (is.environment(owner) && any(vapply(sys.frames(), identical, NA, owner))) {
    stop(sprintf(paste("the reader is in use: %s from inside a read, bind, close or sample of",
                       "the same reader (a handler, an interrupt or a finalizer) is refused"),
                 what), call. = FALSE)
  }
  if (isTRUE(x$closed)) {
    stop(sprintf("the reader is closed: %s after close() is refused", what), call. = FALSE)
  }
  # Undone on the caller's exit, registered first: an interrupt between the two leaves the
  # reader free, never busy for good.
  undo <- bquote(if (identical(.(x)$busy, .(frame))) {
    assign("busy", NULL, envir = .(x))
    assign("interruptible", FALSE, envir = .(x))
  })
  do.call(base::on.exit, list(undo, TRUE, FALSE), envir = frame)
  x$busy <- frame
  x$interruptible <- interruptible
  invisible()
}

# Whether the caller has interrupts suspended. R has no exported way to ask, so this reads it
# from suspendInterrupts()'s own frame, where base keeps it as `suspended`; unknown is taken as
# not suspended.
interrupts_held <- function() {
  suspendInterrupts(isTRUE(dynGet("suspended", ifnotfound = FALSE, inherits = FALSE)))
}

# A read holds interrupts, and runs here, with interrupts as its caller had them, only the steps
# an interrupt can leave whole: a fill, whose batch the shim keeps until R acknowledges it; a
# connection's read, which the shim takes and feeds in one call; the building of a batch not yet
# acknowledged. Everything that moves the reader on runs held, so an interrupt can't land between
# a step and its bookkeeping.
interruptible <- function(x, expr) if (isTRUE(x$interruptible)) allowInterrupts(expr) else expr

#' Reading a batch at a time
#'
#' A reader ([delimited_reader()], [sheet_reader()]) is opened without a plan or with one;
#' `header()` is the header it read as it opened, and `bind()` gives it its plan once, so a
#' plan can name source columns by the header's names. `read_batch()` reads the next batch
#' of up to the reader's `batch_rows` rows, as a [hypertabular_batch], and returns `NULL` once
#' there are no more: at the end of the input, or at a structural failure, which
#' [failure()] then holds. A batch is a copy, and stays good after the reader moves on and
#' after it is closed.
#'
#' `close()` lets go of the reader's native memory and of a connection it opened, now rather
#' than at the next garbage collection. It is safe to call again; a `read_batch()` or `bind()`
#' after it is an R error. A reader is called into once at a time: a `read_batch()`,
#' `bind()` or `close()` from a handler, a finalizer or a connection's own R code that reaches
#' the same reader during one of these is an R error, and the read goes on as it was.
#'
#' An interrupt during `read_batch()` loses nothing: the next `read_batch()` hands over the same
#' rows, numbered the same, and the read goes on. The one thing no R function can guard is the
#' caller's own code: an interrupt landing after `read_batch()`'s last interrupt check, before
#' the batch is assigned, loses that one batch, and shows as a gap in `row` (each batch's first
#' `row` follows the last one's otherwise). A read from a connection whose read can stop
#' part-way (a socket or a URL, not a file or a raw vector) can't resume after an interrupt
#' during that read, since the bytes it had may be gone: every later `read_batch()` is then an R
#' error saying so, and `close()` still works.
#'
#' @param x,con A reader.
#' @param plan A list of columns ([col_bool()] and its siblings), named for the output
#'   columns.
#' @param ... Ignored.
#' @return `header()` returns the header's names exactly as the source holds them, as a
#'   character vector (a name holding a NUL byte is `NA`), `character(0)` for an input with no
#'   record, or `NULL` when no header is declared. `bind()` and `close()` return `x`
#'   invisibly. `read_batch()` returns a `hypertabular_batch` or `NULL`.
#' @family reads
#' @export
#' @examples
#' reader <- delimited_reader(charToRaw("a,b\n1,x\n2,y\n3,z\n"), batch_rows = 2)
#' header(reader)
#' bind(reader, list(col_i32("a"), col_text("b")))
#' while (!is.null(batch <- read_batch(reader))) print(batch$columns$a$value)
#' failure(reader)
#' close(reader)
header <- function(x) {
  check_reader(x)
  x$header
}

#' @rdname header
#' @export
bind <- function(x, plan) {
  check_reader(x)
  # Held throughout: the shim's plan and R's are set together.
  invisible(suspendInterrupts(bind_held(x, plan)))
}

bind_held <- function(x, plan) {
  enter(x, "bind()")
  if (!is.null(x$plan)) {
    stop("the reader already has a plan: a plan is bound once", call. = FALSE)
  }
  plan <- as_plan(plan)
  if (!is.null(x$failure)) {
    # The header could not be read: no read will bind, so a source name has nothing to be
    # found in, and the failure stays a value. read_batch() returns NULL from here on.
    judge_plan(plan, pack_specs(plan, integer(length(plan))))
    x$plan <- plan
    x$names <- resolve_names(plan)
    x$ordinals <- vapply(plan, function(column) {
      if (is.numeric(column$source)) column$source - 1 else NA_real_
    }, 0)
  } else {
    resolved <- if (inherits(x, "hypertabular_delimited_reader")) {
      bind_delimited(x, plan)
    } else {
      bind_sheet(x, plan)
    }
    x$plan <- plan
    x$names <- resolved$names
    x$ordinals <- resolved$ordinals
  }
  invisible(x)
}

#' @rdname header
#' @export
read_batch <- function(x) {
  check_reader(x)
  allowed <- !interrupts_held()
  # Held, but for the steps interruptible() names; its last statement is the acknowledgement,
  # and nothing after it runs while an interrupt can land.
  suspendInterrupts(read_batch_held(x, allowed))
}

read_batch_held <- function(x, allowed) {
  enter(x, "read_batch()", interruptible = allowed)
  raw <- read_raw_batch(x)
  if (is.null(raw)) {
    return(NULL)
  }
  batch <- interruptible(x, new_batch(raw, x))
  # Acknowledged, then a point where an interrupt pending by now lands while read_batch() hasn't
  # returned. `handing` is cleared only past that point, so a later read that finds it set
  # knows the batch never reached the caller, and takes the acknowledgement back
  # (read_raw_batch()): the same rows are handed over again, numbered the same.
  x$handing <- TRUE
  received(x, raw)
  interruptible(x, .Call(C_interrupt_point))
  x$handing <- FALSE
  batch
}

# The batch `raw` is R's: the shim counts its rows and reads on at the next fill, in one call.
# Until this runs, an interrupt leaves the batch with the shim to hand over again. A failure that
# came with the rows ends the read once they're R's.
received <- function(x, raw) {
  if (inherits(x, "hypertabular_delimited_reader")) {
    .Call(C_delimited_ack, x$ptr)
  } else {
    .Call(C_sheet_ack, x$ptr)
  }
  if (!is.null(raw$pending)) x$pending <- raw$pending
  invisible()
}

# The next batch as the C shim wrote it, unclassed, or NULL. It is the shim's until received().
read_raw_batch <- function(x, sample = FALSE) {
  if (isTRUE(x$handing)) {
    if (inherits(x, "hypertabular_delimited_reader")) {
      .Call(C_delimited_unack, x$ptr)
    } else {
      .Call(C_sheet_unack, x$ptr)
    }
    x$pending <- NULL
    x$handing <- FALSE
  }
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
  raw <- if (inherits(x, "hypertabular_delimited_reader")) {
    read_delimited_batch(x, sample)
  } else {
    read_sheet_batch(x, sample)
  }
  if (!is.null(raw)) {
    raw <- list(columns = if (sample) raw else raw$columns,
                line = if (!sample) raw$line, widths = if (!sample) raw$widths,
                first = attr(raw, "first"),
                rows = if (sample) length(raw[[1]] %||% list()) else length(raw$line),
                pending = attr(raw, "pending"))
  }
  raw
}

#' @rdname header
#' @export
close.hypertabular_reader <- function(con, ...) {
  check_reader(con)
  invisible(suspendInterrupts(close_held(con)))
}

close_held <- function(con) {
  if (isTRUE(con$closed)) {
    return(invisible(con))
  }
  x <- con
  enter(x, "close()")
  close_source(x)
  if (!is.null(x$ptr)) {
    if (inherits(x, "hypertabular_delimited_reader")) {
      .Call(C_delimited_close, x$ptr)
    } else {
      .Call(C_sheet_close, x$ptr)
    }
  }
  x$ptr <- NULL
  x$closed <- TRUE
  invisible(con)
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
  bind(reader, plan)
  raws <- list()
  repeat {
    raw <- local({
      x <- reader
      enter(x, "a read")
      raw <- read_raw_batch(x)
      if (!is.null(raw)) {
        raws[[length(raws) + 1L]] <<- raw
        received(x, raw)
      }
      raw
    })
    if (is.null(raw)) break
  }
  new_table(reader, raws, reader$plan, reader$names)
}

# The output names of a plan no read will bind.
resolve_names <- function(plan) {
  vapply(seq_along(plan), function(i) {
    given <- names(plan)
    if (!is.null(given) && !is.na(given[[i]]) && nzchar(given[[i]])) return(given[[i]])
    source <- plan[[i]]$source
    if (is.character(source)) source else paste0("V", source)
  }, "")
}

#' @export
print.hypertabular_reader <- function(x, ...) {
  kind <- if (inherits(x, "hypertabular_delimited_reader")) "delimited" else "sheet"
  state <- if (isTRUE(x$closed)) {
    "closed"
  } else if (!is.null(x$failure)) {
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
