# Run by test-lifecycle.R in a process of its own: interrupts land in reads, from inside at each
# step a read moves on at and from outside at random, and a caller who catches each reads on.
# Every value and every `row` is held to the file. Prints "ok" and the interrupts caught.
library(hypertabular)
sheet_path <- commandArgs(TRUE)[[1]]
ns <- asNamespace("hypertabular")
n <- 20000
path <- tempfile(fileext = ".csv")
writeLines(c("a", as.character(seq_len(n))), path)
want <- as.numeric(seq_len(n))
plan <- list(col_i32("a"))
count <- new.env()
count$caught <- 0
again <- function(e) {
  count$caught <- count$caught + 1
  "again"
}

# Waits out the interrupts still coming, catching each.
settle <- function() {
  repeat {
    quiet <- tryCatch(allowInterrupts({
      Sys.sleep(0.7)
      TRUE
    }), interrupt = again)
    if (isTRUE(quiet)) break
  }
}

# One interrupt, landing as one from outside would, when armed. R acts on a SIGINT at its next
# interrupt check, so one follows: where interrupts are allowed it lands right there, where they
# are held it waits for the next allowed check. No sleep, which would let it through a hold.
point <- get("C_interrupt_point", ns)
flag <- new.env()
flag$armed <- FALSE
signal <- bquote(if (get("armed", envir = .(flag))) {
  assign("armed", FALSE, envir = .(flag))
  tools::pskill(Sys.getpid(), tools::SIGINT)
  .Call(.(point))
})

# Every batch to the end, reading on after each interrupt, armed before the `arm`-th read: the
# values and their `row`s, and what refused the read, if anything did. The script runs with
# interrupts held, so one lands only in a read or a wait that catches it, and, with `gap`, in an
# interrupt check of the caller's own right after read_batch() returns: one still pending as a
# read returns would land there and take the batch with it.
drain <- function(reader, arm = 0, gap = FALSE) {
  values <- list()
  rows <- list()
  k <- 0
  said <- NULL
  repeat {
    k <- k + 1
    if (k == arm) flag$armed <- TRUE
    b <- tryCatch(
      allowInterrupts({
        got <- read_batch(reader)
        if (gap) .Call(point)
        got
      }),
      interrupt = again, error = function(e) conditionMessage(e)
    )
    if (identical(b, "again")) next
    if (is.character(b)) {
      said <- b
      break
    }
    if (is.null(b)) break
    values[[length(values) + 1L]] <- as.numeric(b$columns[[1]]$value)
    rows[[length(rows) + 1L]] <- as.numeric(b$row)
  }
  list(values = unlist(values) %||% numeric(), rows = unlist(rows) %||% numeric(), said = said)
}
`%||%` <- function(a, b) if (is.null(a)) b else a

# Whole and exact, or else an error naming what differs.
exact <- function(got, want, label) {
  if (!is.null(got$said)) stop(label, ": refused: ", got$said, call. = FALSE)
  if (!identical(got$values, want) || !identical(got$rows, as.numeric(seq_along(want)))) {
    stop(sprintf("%s: %d values, %d of them the file's in order, rows %s", label,
                 length(got$values), sum(got$values == seq_along(got$values)),
                 if (identical(got$rows, as.numeric(seq_along(got$rows)))) "in order" else "wrong"),
         call. = FALSE)
  }
}

# An interrupt at a step of a package function, armed before the first read and the third.
at_step <- function(what, where, open, want, label) {
  args <- list(what, where = ns, print = FALSE)
  if (identical(where, "exit")) {
    args$exit <- signal
  } else {
    args$at <- where
    args$tracer <- signal
  }
  suppressMessages(do.call(trace, args, quote = TRUE))
  on.exit(suppressMessages(untrace(what, where = ns)))
  for (arm in c(1, 3)) {
    before <- count$caught
    reader <- open()
    exact(drain(reader, arm, gap = TRUE), want, sprintf("%s, armed at read %d", label, arm))
    close(reader)
    if (count$caught == before) stop(label, ": the interrupt never landed", call. = FALSE)
  }
}

main <- function() {
  from_file <- function() {
    delimited_reader(path, plan = plan, batch_rows = 1000, buffer_bytes = 4096)
  }
  at_step("received", 1, from_file, want, "before the acknowledgement")
  at_step("received", "exit", from_file, want, "after the acknowledgement")
  at_step("read_raw_batch", "exit", from_file, want, "after the fill")
  at_step("new_batch", "exit", from_file, want, "after the batch is built")
  at_step("feed", 1, from_file, want, "before a connection's read")
  at_step("feed", "exit", from_file, want, "after a connection's read")
  at_step("readBin", "exit", from_file, want, "as readBin returns its chunk")
  book <- workbook(sheet_path)
  sheet_want <- as.numeric(read_sheet(book, 1, list(col_text(1)))$columns[[1]]$ok)
  from_sheet <- function() sheet_reader(book, 1, list(col_text(1)), batch_rows = 1)
  for (where in list(1, "exit")) {
    suppressMessages(do.call(trace, c(list("received", where = ns, print = FALSE),
                                      if (identical(where, "exit")) list(exit = signal)
                                      else list(at = where, tracer = signal)), quote = TRUE))
    before <- count$caught
    reader <- from_sheet()
    got <- drain(reader, 2, gap = TRUE)
    if (!is.null(got$said) || length(got$rows) != length(sheet_want) ||
          !identical(got$rows, as.numeric(seq_along(sheet_want))) || count$caught == before) {
      stop("a sheet, interrupted at the acknowledgement: rows ", length(got$rows), call. = FALSE)
    }
    close(reader)
    suppressMessages(untrace("received", where = ns))
  }

  # A bind an interrupt reaches still binds: the shim's plan and R's are set together.
  suppressMessages(trace("bind_delimited", exit = signal, where = ns, print = FALSE))
  reader <- delimited_reader(path, batch_rows = 1000)
  flag$armed <- TRUE
  before <- count$caught
  tryCatch(allowInterrupts(bind(reader, plan)), interrupt = again)
  settle()
  suppressMessages(untrace("bind_delimited", where = ns))
  if (count$caught == before || is.null(reader$plan)) {
    stop("bind: the interrupt or the plan went missing", call. = FALSE)
  }
  exact(drain(reader), want, "after an interrupted bind")
  close(reader)

  # A connection whose read can stop part-way (a pipe) can't resume after an interrupt in that
  # read: every later read refuses, saying so, and close() still works.
  # The wait lets the interrupt through inside readBin, as a socket's read waiting on the network
  # would.
  waiting <- bquote({
    .(signal)
    Sys.sleep(0.01)
  })
  suppressMessages(trace("readBin", tracer = waiting, where = ns, print = FALSE))
  piped <- pipe(paste("cat", shQuote(path)), "rb")
  reader <- delimited_reader(piped, plan = plan, batch_rows = 1000, buffer_bytes = 4096)
  got <- drain(reader, 3)
  suppressMessages(untrace("readBin", where = ns))
  if (!grepl("an interrupted read can't resume from here", got$said %||% "") ||
        !identical(got$values, as.numeric(seq_along(got$values))) ||
        !identical(got$rows, as.numeric(seq_along(got$rows)))) {
    stop("a pipe interrupted mid-read: ", got$said %||% "read on", call. = FALSE)
  }
  close(reader)
  close(piped)

  # A reader whose busy mark outlived its frame (an interrupt during the undoing) isn't held by
  # it: the mark is stale, and the read and close() go on.
  reader <- from_file()
  reader$busy <- new.env()
  exact(drain(reader), want, "a stale busy mark")
  close(reader)

  # Interrupts from outside, at random, many to a read in batches of 50 through a 512-byte buffer.
  set.seed(20261108)
  storm <- function() {
    loop <- "(for i in $(seq 1 30); do sleep 0.0%02d; kill -INT %d 2>/dev/null; done)"
    system(sprintf(paste(loop, ">/dev/null 2>&1 &"), sample(5:20, 1), Sys.getpid()))
  }
  stormed <- count$caught
  for (trial in 1:6) {
    reader <- delimited_reader(path, plan = plan, batch_rows = 50, buffer_bytes = 512)
    storm()
    got <- drain(reader)
    settle()
    exact(got, want, sprintf("storm, trial %d", trial))
    close(reader)
  }
  if (count$caught == stormed) stop("the storm never landed", call. = FALSE)

  # read_delimited() interrupted leaves no connection open.
  connections <- nrow(showConnections(all = TRUE))
  for (trial in 1:4) {
    storm()
    tryCatch(allowInterrupts(read_delimited(path, plan, buffer_bytes = 4096)), interrupt = again)
    settle()
  }
  invisible(gc())
  if (nrow(showConnections(all = TRUE)) != connections) stop("a connection was left open")
  cat("ok", count$caught, "\n")
}
suspendInterrupts(main())
