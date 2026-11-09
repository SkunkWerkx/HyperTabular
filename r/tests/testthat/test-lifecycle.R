# A reader's life: close, repeated and early; what a batch keeps after its reader is gone; a
# reader called into from inside its own read (a connection's read, a condition handler, an
# on.exit, a finalizer) refused cleanly; an interrupt mid-read losing nothing and leaving no
# connection open; garbage collection under gctorture; and the two packages loaded in either
# order.

csv <- function(text) charToRaw(enc2utf8(text))

test_that("close is safe twice, and a read after it is an error", {
  reader <- delimited_reader(csv("a,b\n1,x\n2,y\n"), plan = list(col_i32("a")), batch_rows = 1)
  first <- read_batch(reader)
  expect_identical(close(reader), reader)
  expect_silent(close(reader))
  expect_error(read_batch(reader), "the reader is closed")
  expect_error(bind(reader, list(col_i32(1))), "the reader is closed")
  expect_identical(header(reader), c("a", "b"))
  expect_output(print(reader), "closed")
  expect_identical(first$columns[[1]]$value, 1)
  # A sheet, and its workbook in either order.
  path <- testthat::test_path("corpus", "workbook", "basic.xlsx")
  book <- workbook(path)
  sheet <- sheet_reader(book, 1, batch_rows = 1)
  bind(sheet, list(col_text(1)))
  close(book)
  close(book)
  expect_error(sheet_reader(book, 1), "the workbook is closed")
  # The sheet holds what it needs of the workbook, and reads on.
  rows <- 0
  while (!is.null(batch <- read_batch(sheet))) rows <- rows + length(batch$line)
  expect_identical(rows, as.numeric(nrow(as.data.frame(read_sheet(path, 1)))))
  close(sheet)
  close(sheet)
  expect_error(read_batch(sheet), "the reader is closed")
  book <- workbook(path)
  sheet <- sheet_reader(book, 1)
  close(sheet)
  expect_s3_class(read_sheet(book, 1), "hypertabular_table")
  close(book)
})

test_that("a batch stays good after its reader is closed and collected", {
  reader <- delimited_reader(csv("a,b\n1,x\n2,é\n"),
                             plan = list(col_i32("a"), col_text("b"), col_text("b", raw = TRUE)))
  batch <- read_batch(reader)
  copy <- unserialize(serialize(batch, NULL))
  close(reader)
  rm(reader)
  invisible(gc())
  invisible(gc())
  expect_identical(batch, copy)
  expect_identical(batch$columns[[2]]$value, c("x", "é"))
  expect_identical(problems(batch), problems(copy))
})

test_that("a reader whose read ends early closes the connection it opened", {
  path <- tempfile(fileext = ".csv")
  on.exit(unlink(path), add = TRUE)
  writeLines(c("a", "1", "2"), path)
  before <- nrow(showConnections(all = TRUE))
  # A plan error, an error from a connection, and a read left unfinished, then closed.
  expect_error(read_delimited(path, list(col_i32("missing"))), "not in the header")
  expect_identical(nrow(showConnections(all = TRUE)), before)
  reader <- delimited_reader(path, batch_rows = 1, plan = list(col_i32(1)))
  read_batch(reader)
  expect_identical(nrow(showConnections(all = TRUE)), before + 1L)
  close(reader)
  expect_identical(nrow(showConnections(all = TRUE)), before)
})

# readBin calling back into R from inside a read, as a connection that runs R code would: the
# callback in `hook$f` runs once, on the read after it is set.
hook <- new.env()
hook$f <- NULL
trace_reads <- function() {
  tracer <- bquote({
    f <- get("f", envir = .(hook))
    if (!is.null(f)) {
      assign("f", NULL, envir = .(hook))
      f()
    }
  })
  suppressMessages(trace("readBin", tracer = tracer, print = FALSE,
                         where = asNamespace("hypertabular")))
}
untrace_reads <- function() {
  hook$f <- NULL
  suppressMessages(untrace("readBin", where = asNamespace("hypertabular")))
}

# The values a whole read gives, through `read`, of a file of `n` rows.
lines_file <- function(n) {
  path <- tempfile(fileext = ".csv")
  writeLines(c("a,b", paste0(seq_len(n), ",x", seq_len(n))), path)
  path
}

read_values <- function(reader) {
  out <- list()
  while (!is.null(batch <- read_batch(reader))) out[[length(out) + 1L]] <- batch$columns[[1]]$value
  unlist(out)
}

test_that("a reader called into from inside its own read is refused, and the read goes on", {
  path <- lines_file(200)
  on.exit(unlink(path), add = TRUE)
  on.exit(untrace_reads(), add = TRUE)
  trace_reads()
  plan <- list(col_i32("a"))
  open <- function() delimited_reader(path, plan = plan, batch_rows = 7, buffer_bytes = 16)
  # The negative control: nothing calls in, and the read is whole.
  expect_identical(read_values(open()), as.numeric(1:200))
  inner <- function(reader, call) {
    function() {
      seen <- tryCatch({
        call(reader)
        "allowed"
      }, error = function(e) conditionMessage(e))
      hook$said <- c(hook$said, seen)
    }
  }
  calls <- list(read = read_batch, close = close, bind = function(r) bind(r, plan),
                suggest = function(r) hypertabular:::sample_read(r, TRUE))
  for (name in names(calls)) {
    hook$said <- character()
    reader <- open()
    first <- read_batch(reader)$columns[[1]]$value
    hook$f <- inner(reader, calls[[name]])
    values <- c(first, read_values(reader))
    expect_match(hook$said, "the reader is in use", label = name)
    expect_identical(values, as.numeric(1:200), label = name)
    close(reader)
  }
  # From a condition handler: a warning the connection signals.
  reader <- open()
  hook$said <- character()
  hook$f <- function() warning("a slow disk")
  values <- withCallingHandlers(read_values(reader), warning = function(w) {
    hook$said <- tryCatch({
      read_batch(reader)
      "allowed"
    }, error = function(e) conditionMessage(e))
    invokeRestart("muffleWarning")
  })
  expect_match(hook$said, "the reader is in use")
  expect_identical(values, as.numeric(1:200))
  # From an on.exit, and from a finalizer the collector runs mid-read.
  reader <- open()
  hook$said <- character()
  hook$f <- function() {
    on.exit(hook$said <- tryCatch({
      close(reader)
      "allowed"
    }, error = function(e) conditionMessage(e)))
    NULL
  }
  expect_identical(read_values(reader), as.numeric(1:200))
  expect_match(hook$said, "the reader is in use")
  reader <- open()
  hook$said <- character()
  hook$f <- function() {
    local({
      e <- new.env()
      reg.finalizer(e, function(e) {
        hook$said <- tryCatch({
          read_batch(reader)
          "allowed"
        }, error = function(e) conditionMessage(e))
      })
    })
    invisible(gc())
  }
  expect_identical(read_values(reader), as.numeric(1:200))
  expect_match(hook$said, "the reader is in use")
})

test_that("garbage collected at every allocation, a read is whole", {
  input <- csv("a,b,c\n1,x,2026-01-07\n2,é,\n3,,bad\n")
  plan <- list(a = col_i32("a"), b = col_text("b"), raw = col_text("b", raw = TRUE),
               c = col_date("c"))
  want <- read_delimited(input, plan)
  path <- testthat::test_path("corpus", "workbook", "basic.xlsx")
  sheet_want <- read_sheet(path, 1, list(col_text(1), col_f64(2)))
  # A file is read through its connection a chunk at a time, through the shim's readBin call.
  file <- tempfile(fileext = ".csv")
  writeBin(input, file)
  on.exit(unlink(file))
  gctorture(TRUE)
  got <- read_delimited(input, plan)
  from_file <- read_delimited(file, plan, buffer_bytes = 16)
  sheet_got <- read_sheet(path, 1, list(col_text(1), col_f64(2)))
  p <- problems(got)
  gctorture(FALSE)
  expect_identical(got, want)
  expect_identical(from_file, want)
  expect_identical(sheet_got, sheet_want)
  expect_identical(p, problems(want))
})

rscript <- function(code) {
  out <- suppressWarnings(system2(file.path(R.home("bin"), "Rscript"), c("-e", shQuote(code)),
                                  stdout = TRUE, stderr = TRUE,
                                  env = paste0("R_LIBS=", paste(.libPaths(), collapse = ":"))))
  out
}

test_that("both packages load in either order, in fresh processes", {
  skip_on_cran()
  check <- c(
    "t <- hypertabular::read_delimited(charToRaw('a\\n1,5\\n'), list(hypertabular::col_f64('a',",
    "hypercast::num_format(',', '.'))), hypertabular::delimited_dialect(';'))",
    "stopifnot(identical(t$columns[[1]]$value, 1.5))",
    "stopifnot(identical(hypercast::cast_i32('7', hypercast::num_format())$value, 7))",
    "cat(format(hypertabular::hypertabular_native_version()),",
    "format(hypercast::hypercast_native_version()), '\\n')"
  )
  check <- paste(check, collapse = "\n")
  first <- rscript(paste("library(hypercast); library(hypertabular)", check, sep = "\n"))
  second <- rscript(paste("library(hypertabular); library(hypercast)", check, sep = "\n"))
  third <- rscript(paste("invisible(loadNamespace('hypertabular'))", check, sep = "\n"))
  want <- paste(packageVersion("hypertabular"), hypercast::hypercast_native_version(), "")
  expect_identical(first, want)
  expect_identical(second, want)
  expect_identical(third, want)
})

test_that("an interrupt during a read hands every row over once, numbered right, or refuses", {
  skip_on_cran()
  skip_on_os("windows")
  # In a process of its own (interrupted-read.R): one interrupt at each step a read moves on
  # at, from a file, in batches of 1000 through a 4 KiB buffer, and a sheet; one in a bind; one
  # inside a pipe's read, which then refuses; a stale busy mark; then interrupts from outside at
  # random, many to a read in batches of 50 through a 512-byte buffer; and read_delimited()
  # interrupted, leaving no connection open. Every value and `row` is held to the file.
  script <- normalizePath(testthat::test_path("interrupted-read.R"))
  sheet <- normalizePath(testthat::test_path("corpus", "workbook", "basic.xlsx"))
  out <- suppressWarnings(system2(file.path(R.home("bin"), "Rscript"),
                                  c(shQuote(script), shQuote(sheet)), stdout = TRUE,
                                  stderr = TRUE,
                                  env = paste0("R_LIBS=", paste(.libPaths(), collapse = ":"))))
  expect_match(out[length(out)], "^ok [1-9][0-9]* $", info = paste(out, collapse = "\n"))
})

test_that("only a bounded local read is held from interrupts", {
  whole <- function(con) {
    on.exit(close(con))
    hypertabular:::whole_reads(con)
  }
  path <- tempfile()
  writeLines("a", path)
  expect_true(whole(file(path, "rb")))
  expect_true(whole(gzfile(path, "rb")))
  expect_true(whole(rawConnection(as.raw(1))))
  expect_false(whole(file("stdin")))
  skip_on_os("windows")
  fifo <- tempfile()
  skip_if_not(system2("mkfifo", shQuote(fifo)) == 0, "no mkfifo")
  # Unopened: opening a FIFO would wait for a writer.
  expect_false(whole(file(fifo, raw = TRUE)))
  unlink(c(path, fifo))
})
