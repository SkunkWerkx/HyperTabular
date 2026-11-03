# Replays the two conformance corpus files at the repository root, the same files the Rust
# core and every other binding replay, through the public API.
#
# corpus/delimited.json: every case from a raw vector, from connections read through buffers
# too small for a record, from a file and from an unopened connection, in batches of one row,
# two rows and many; with the plan bound after the header, by position and by header name;
# and with every buffer the core works in starting at one element. How the input is cut up
# must not change the answer.
#
# corpus/workbook.json and the packages it names under corpus/workbook/: every sheet of every
# package read whole, through read_sheet() with the sheet opened by its position; and a batch
# of seven rows at a time through sheet_reader() and bind(), the sheet opened by name where
# its name is unique and the plan naming its sources by header name where the header has them
# once, with every buffer starting at one element, so the grow-and-keep path runs mid-part
# throughout. The two encrypted files are refused as they open.
#
# The corpus pins fault spans as byte offsets into the cell's text; this binding presents them
# in the units substr() counts `raw` in, characters for UTF-8 text, so each pinned span is
# re-expressed in characters before comparing.

# A dependency skip, not a corpus skip: jsonlite is in Suggests, and CRAN's noSuggests flavour
# runs the tests without it. With jsonlite present, a missing corpus still errors.
testthat::skip_if_not_installed("jsonlite")

# Shared: where the corpus is, how it spells a plan, a value and a failure, and how a column
# of a read is held to it. A column is compared whole, field by field, and every
# disagreement is named; each replay expects the list of them to be empty.

corpus_dir <- function() {
  staged <- testthat::test_path("corpus")
  if (dir.exists(staged)) {
    return(staged)
  }
  here <- getwd()
  for (i in 1:6) {
    here <- file.path(here, "..")
    candidate <- file.path(here, "corpus")
    if (file.exists(file.path(candidate, "delimited.json"))) {
      return(normalizePath(candidate))
    }
  }
  stop("corpus not found at ", staged, " or above ", getwd())
}

# A corpus file, its integer values read as their exact text: a u64 past 2^53 has no exact
# double. A missing or empty file is an error, never a skip.
read_corpus <- function(name) {
  path <- file.path(corpus_dir(), name)
  if (!file.exists(path) || file.size(path) == 0) {
    stop("corpus file missing or empty: ", path)
  }
  json <- paste(readLines(path, encoding = "UTF-8", warn = FALSE), collapse = "\n")
  json <- gsub("\"(value|magnitude)\":( ?)(-?[0-9]+)(?=[,}\\s])", "\"\\1\":\\2\"\\3\"", json,
               perl = TRUE)
  cases <- jsonlite::fromJSON(json, simplifyVector = FALSE)
  if (!length(cases)) stop("corpus file has no cases: ", path)
  cases
}

corpus_precisions <- c("seconds", "milliseconds", "microseconds", "nanoseconds")
corpus_orders <- c("year_month_day", "month_day_year", "day_month_year")
corpus_epochs <- c("y1900", "y1904")

format_of <- function(spec) {
  if (is.null(spec)) {
    return(NULL)
  }
  opt <- function(name) if (is.null(spec[[name]])) "" else spec[[name]]
  hypercast::num_format(spec$decimal_sep, spec$group_sep, spec$flags, currency = opt("currency"),
                        currency_decimal_sep = opt("currency_decimal_sep"),
                        currency_group_sep = opt("currency_group_sep"),
                        minus_sign = opt("minus_sign"), plus_sign = opt("plus_sign"),
                        percent = opt("percent"), digit_zero = opt("digit_zero"))
}

# A corpus plan entry as a column; `by_name` reads the source by its header name instead of
# its position where the header has that name once, at or before the position.
column_of <- function(entry, header = NULL, by_name = FALSE) {
  source <- entry$ordinal + 1L
  if (by_name && !is.null(header) && source <= length(header)) {
    name <- header[[source]]
    if (!is.na(name) && nzchar(name) && sum(header == name, na.rm = TRUE) == 1L) source <- name
  }
  switch(entry$door,
    unix = col_unix(source, corpus_precisions[[entry$precision]]),
    excel_serial = col_excel_serial(source, corpus_epochs[[entry$epoch]]),
    date_ordered = col_date(source, corpus_orders[[entry$order]]),
    datetime = col_datetime(source, corpus_orders[[entry$order]]),
    i8 = , i16 = , i32 = , i64 = , u8 = , u16 = , u32 = , u64 = , f32 = , f64 = ,
    decimal = get(paste0("col_", entry$door))(source, format_of(entry$format)),
    get(paste0("col_", entry$door))(source)
  )
}

plan_of <- function(case, header = NULL, by_name = FALSE) {
  lapply(case$plan, column_of, header = header, by_name = by_name)
}

# The wall clock text a civil date-time comes out as, built from a cell's fields.
civil_text <- function(cell) {
  secs <- cell$nanos_of_day %/% 1e9
  frac <- cell$nanos_of_day %% 1e9
  text <- sprintf("%04d-%02d-%02d %02d:%02d:%02d", cell$year, cell$month, cell$day,
                  secs %/% 3600, secs %/% 60 %% 60, secs %% 60)
  if (frac > 0) text <- paste0(text, ".", sub("0+$", "", sprintf("%09.0f", frac)))
  text
}

as_f32 <- function(x) readBin(writeBin(as.numeric(x), raw(), size = 4L), "double", size = 4L)

days <- function(cell) {
  as.numeric(as.Date(sprintf("%04d-%02d-%02d", cell$year, cell$month, cell$day)))
}

# What a door's verdict holds for a cell that cast, field by field, as the corpus says it.
expected_fields <- function(door, cell) {
  switch(door,
    bool = list(value = cell$value),
    i8 = , i16 = , u8 = , u16 = list(value = as.integer(cell$value)),
    i32 = , u32 = , f64 = list(value = as.numeric(cell$value)),
    f32 = list(value = as_f32(cell$value)),
    i64 = , u64 = , decimal = list(value = cell$value),
    uuid = list(value = sub("^(.{8})(.{4})(.{4})(.{4})(.{12})$", "\\1-\\2-\\3-\\4-\\5",
                            tolower(cell$value))),
    timestamp = , unix = , excel_serial = , duration = list(seconds = as.numeric(cell$seconds),
                                                            nanos = as.integer(cell$nanos)),
    date = , date_ordered = list(value = days(cell)),
    datetime = list(value = civil_text(cell), date = days(cell),
                    nanos_of_day = as.numeric(cell$nanos_of_day)),
    time = list(nanos = as.numeric(cell$nanos)),
    text = list(value = cell$text),
    stop("unknown door ", door)
  )
}

# Characters in the first n bytes of x's UTF-8.
chars_in <- function(bytes, n) sum(bitwAnd(as.integer(bytes[seq_len(n)]), 0xC0) != 0x80)

# Every way verdict `v` (a plan column of door `door`) disagrees with `cells`, the corpus's
# cells for that column, one string per disagreement.
column_mismatches <- function(v, door, cells, label) {
  out <- character()
  miss <- function(i, what, got, want) {
    sprintf("%s, row %d (%s): %s is %s, want %s", label, i, door, what,
            paste(format(got), collapse = " "), paste(format(want), collapse = " "))
  }
  if (nrow(v) != length(cells)) {
    return(sprintf("%s: %d rows, want %d", label, nrow(v), length(cells)))
  }
  plain <- unclass(as.data.frame(v))
  for (i in seq_along(cells)) {
    cell <- cells[[i]]
    want_ok <- identical(cell$expect, "ok")
    if (!identical(plain$ok[[i]], want_ok)) {
      out <- c(out, miss(i, "ok", plain$ok[[i]], paste(want_ok, cell$expect, plain$raw[[i]])))
      next
    }
    if (want_ok) {
      for (field in names(want <- expected_fields(door, cell))) {
        got <- plain[[field]][[i]]
        if (!identical(unclass(got), want[[field]])) {
          out <- c(out, miss(i, field, got, want[[field]]))
        }
      }
      next
    }
    if (!identical(as.character(plain$reason[[i]]), cell$expect)) {
      out <- c(out, miss(i, "reason", plain$reason[[i]], cell$expect))
    }
    span <- c(0L, 0L)
    if (!is.null(cell$fault)) {
      if (!identical(plain$raw[[i]], cell$raw)) {
        out <- c(out, miss(i, "raw", plain$raw[[i]], cell$raw))
      }
      bytes <- charToRaw(enc2utf8(cell$raw))
      start <- cell$fault[[1]]
      span <- as.integer(c(chars_in(bytes, start),
                           chars_in(bytes, start + cell$fault[[2]]) - chars_in(bytes, start)))
    }
    got <- c(plain$offset[[i]], plain$length[[i]])
    if (!identical(got, span)) out <- c(out, miss(i, "span", got, span))
  }
  out
}

# Every way table `t` disagrees with `case`'s rows (and its row numbers, when it has them).
table_mismatches <- function(t, case, label) {
  rows <- case$rows
  out <- character()
  if (length(t$line) != length(rows)) {
    out <- sprintf("%s: %d rows, want %d", label, length(t$line), length(rows))
  }
  if (!is.null(case$numbers) && !identical(t$line, as.numeric(unlist(case$numbers)))) {
    out <- c(out, sprintf("%s: row numbers %s, want %s", label, paste(t$line, collapse = " "),
                          paste(unlist(case$numbers), collapse = " ")))
  }
  if (length(out)) {
    return(out)
  }
  for (j in seq_along(case$plan)) {
    cells <- lapply(rows, `[[`, j)
    out <- c(out, column_mismatches(t$columns[[j]], case$plan[[j]]$door, cells,
                                    sprintf("%s, column %d", label, j)))
  }
  out
}

failure_mismatches <- function(failure, case, label) {
  want <- case$failure
  if (is.null(want)) {
    if (is.null(failure)) return(character())
    return(sprintf("%s: unexpected failure: %s", label, format(failure)))
  }
  if (is.null(failure)) {
    return(sprintf("%s: no failure, want %s", label, want$kind))
  }
  got <- list(failure$kind, failure$record, failure$line, failure$byte)
  wanted <- list(want$kind, as.numeric(want$record), as.numeric(want$line), as.numeric(want$byte))
  if (!is.null(want$expected)) {
    got <- c(got, failure$expected, failure$found)
    wanted <- c(wanted, as.numeric(want$expected), as.numeric(want$found))
  }
  if (identical(got, wanted)) character() else sprintf("%s: failure %s, want %s", label,
                                                       paste(got, collapse = " "),
                                                       paste(wanted, collapse = " "))
}

# The delimited corpus.

dialect_of <- function(case) {
  d <- case$dialect
  delimited_dialect(d$separator, quoting = d$quoting, has_header = d$has_header,
                    skip_blank_lines = d$skip_blank_lines)
}

# Reads `case` through `open` (a function of the dialect returning a reader with no plan
# bound), checks the header, binds the plan and reads every batch; returns the disagreements.
replay <- function(case, label, open, batch_rows, by_name = FALSE) {
  reader <- open(dialect_of(case))
  out <- character()
  want_header <- if (is.null(case$header)) NULL else as.character(unlist(case$header))
  if (!identical(header(reader), want_header)) {
    out <- sprintf("%s: header %s, want %s", label, paste(header(reader), collapse = "|"),
                   paste(want_header, collapse = "|"))
  }
  bind(reader, plan_of(case, header(reader), by_name))
  raws <- list()
  while (!is.null(batch <- read_batch(reader))) {
    if (length(batch$line) < 1L || length(batch$line) > batch_rows) {
      out <- c(out, sprintf("%s: a batch of %d rows", label, length(batch$line)))
    }
    raws[[length(raws) + 1L]] <- batch
  }
  # A structural failure is final: every later read is NULL, and the failure stays.
  first <- failure(reader)
  if (!is.null(read_batch(reader)) || !identical(failure(reader), first)) {
    out <- c(out, sprintf("%s: a read after the end changed something", label))
  }
  t <- list(line = unlist(lapply(raws, `[[`, "line")),
            columns = lapply(seq_along(case$plan), function(j) {
              do.call(rbind, lapply(raws, function(b) b$columns[[j]]))
            }))
  if (is.null(t$line)) t$line <- numeric()
  if (length(case$rows)) {
    out <- c(out, table_mismatches(t, case, label))
  } else if (length(t$line)) {
    out <- c(out, sprintf("%s: %d rows, want none", label, length(t$line)))
  }
  c(out, failure_mismatches(failure(reader), case, label))
}

delimited_cases <- read_corpus("delimited.json")

test_that("the corpus has its cases, and reaches every door", {
  expect_gte(length(delimited_cases), 30L)
  doors <- unique(unlist(lapply(delimited_cases, function(case) {
    vapply(case$plan, `[[`, "", "door")
  })))
  expect_setequal(doors, names(hypertabular:::door_codes))
})

for (case in delimited_cases) {
  test_that(case$name, {
    input <- charToRaw(enc2utf8(case$input))
    path <- tempfile(fileext = ".csv")
    writeBin(input, path)
    on.exit(unlink(path), add = TRUE)
    out <- character()
    # A raw connection is open from the start, so the caller closes it, not the reader.
    cons <- list()
    on.exit(for (con in cons) close(con), add = TRUE)
    for (stingy in c(FALSE, TRUE)) {
      old <- options(hypertabular.stingy = stingy)
      for (rows in c(1L, 2L, 1024L)) {
        tag <- sprintf("(%s rows a batch%s)", rows, if (stingy) ", stingy" else "")
        out <- c(out, replay(case, paste("memory", tag), function(d) {
          delimited_reader(input, d, batch_rows = rows)
        }, rows))
        for (bytes in c(1L, 5L, 64L)) {
          out <- c(out, replay(case, sprintf("connection through %d bytes %s", bytes, tag),
                               function(d) {
                                 cons[[length(cons) + 1L]] <<- con <- rawConnection(input)
                                 delimited_reader(con, d, batch_rows = rows, buffer_bytes = bytes)
                               }, rows))
        }
      }
      out <- c(out, replay(case, "file through 7 bytes, by name", function(d) {
        delimited_reader(path, d, batch_rows = 2L, buffer_bytes = 7L)
      }, 2L, by_name = TRUE))
      # An unopened connection: the reader opens it, and closes it once the read ends.
      out <- c(out, replay(case, "unopened file connection", function(d) {
        delimited_reader(file(path), d, batch_rows = 3L, buffer_bytes = 3L)
      }, 3L))
      options(old)
    }
    # The one-call read agrees with the batch-at-a-time one.
    t <- read_delimited(input, plan_of(case), dialect_of(case))
    if (length(case$rows)) out <- c(out, table_mismatches(t, case, "read_delimited"))
    out <- c(out, failure_mismatches(t$failure, case, "read_delimited"))
    expect_identical(out, character())
  })
}

# The workbook corpus.

workbook_cases <- read_corpus("workbook.json")
books <- new.env()

book_of <- function(file) {
  if (is.null(books[[file]])) books[[file]] <- workbook(file.path(corpus_dir(), file))
  books[[file]]
}

# The disagreements of one read of `case`.
replay_sheet <- function(case, label, stingy) {
  old <- options(hypertabular.stingy = stingy)
  on.exit(options(old), add = TRUE)
  path <- file.path(corpus_dir(), case$file)
  book <- if (stingy) workbook(path) else book_of(case$file)
  if (is.null(case$sheets)) {
    # An encrypted file: refused as it opens, a value, not an error.
    if (!is.null(book$sheets) && nrow(book$sheets)) {
      return(sprintf("%s: an unreadable workbook lists sheets", label))
    }
    return(c(failure_mismatches(book$failure, case, label),
             failure_mismatches(read_sheet(path)$failure, case, paste(label, "read_sheet"))))
  }
  out <- character()
  if (!is.null(book$failure)) {
    return(sprintf("%s: the workbook failed to open: %s", label, format(book$failure)))
  }
  want_sheets <- data.frame(
    name = vapply(case$sheets, function(s) s$name, ""),
    hidden = vapply(case$sheets, function(s) s$hidden, FALSE)
  )
  if (!identical(book$sheets, want_sheets)) out <- c(out, sprintf("%s: sheets differ", label))
  if (!identical(book$format, case$format)) out <- c(out, sprintf("%s: format", label))
  if (!identical(book$date_system, corpus_epochs[[case$epoch]])) {
    out <- c(out, sprintf("%s: date system", label))
  }
  index <- case$sheet + 1L
  options <- case$options
  want_header <- if (options$has_header) as.character(unlist(case$header))
  if (!stingy) {
    t <- read_sheet(book, index, plan_of(case), has_header = options$has_header,
                    skip_empty_rows = options$skip_empty_rows)
    header <- t$header
  } else {
    name <- want_sheets$name[[index]]
    which <- if (sum(want_sheets$name == name) == 1L) name else index
    reader <- sheet_reader(book, which, has_header = options$has_header,
                           skip_empty_rows = options$skip_empty_rows, batch_rows = 7L)
    header <- header(reader)
    bind(reader, plan_of(case, header, by_name = TRUE))
    raws <- list()
    while (!is.null(batch <- read_batch(reader))) {
      if (length(batch$line) > 7L) out <- c(out, sprintf("%s: a batch too long", label))
      raws[[length(raws) + 1L]] <- unclass(batch)
    }
    t <- hypertabular:::new_table(reader, list(), list(), character())
    t$line <- unlist(lapply(raws, `[[`, "line"))
    if (is.null(t$line)) t$line <- numeric()
    t$columns <- lapply(seq_along(case$plan), function(j) {
      do.call(rbind, lapply(raws, function(b) b$columns[[j]]))
    })
    t$failure <- failure(reader)
  }
  if (!identical(header, want_header)) {
    out <- c(out, sprintf("%s: header %s, want %s", label, paste(header, collapse = "|"),
                          paste(want_header, collapse = "|")))
  }
  if (length(case$rows)) {
    out <- c(out, table_mismatches(t, case, label))
  } else if (length(t$line)) {
    out <- c(out, sprintf("%s: %d rows, want none", label, length(t$line)))
  }
  c(out, failure_mismatches(t$failure, case, label))
}

test_that("the corpus has its cases, over every package, through every door", {
  expect_gte(length(workbook_cases), 240L)
  files <- unique(vapply(workbook_cases, `[[`, "", "file"))
  expect_true(all(file.exists(file.path(corpus_dir(), files))))
  doors <- unique(unlist(lapply(workbook_cases, function(case) {
    vapply(case$plan, `[[`, "", "door")
  })))
  expect_setequal(doors, names(hypertabular:::door_codes))
})

for (case in workbook_cases) {
  test_that(case$name, {
    out <- c(replay_sheet(case, paste(case$name, "(whole)"), FALSE),
             replay_sheet(case, paste(case$name, "(stingy, 7 rows a batch)"), TRUE))
    expect_identical(out, character())
  })
}

# Nothing signalled: no read signals an R condition for anything in its data. Every
# truncation of every delimited corpus input, and truncated and corrupted workbooks, come back
# as verdicts and failures; any error or warning a read signals is collected and fails.

# The conditions `expr` signals, as messages.
signalled <- function(expr) {
  seen <- character()
  withCallingHandlers(
    tryCatch(expr, error = function(e) seen <<- c(seen, conditionMessage(e))),
    warning = function(w) {
      seen <<- c(seen, conditionMessage(w))
      invokeRestart("muffleWarning")
    }
  )
  seen
}

# Every sheet of a workbook, every column read as text and as an f64, whole.
read_every_sheet <- function(bytes) {
  book <- workbook(bytes)
  for (i in seq_len(nrow(book$sheets))) {
    reader <- sheet_reader(book, i, has_header = FALSE, batch_rows = 3L)
    bind(reader, list(col_text(1), col_f64(2), col_date(3), col_text(30)))
    while (!is.null(read_batch(reader))) NULL
  }
}

test_that("no truncation of a delimited corpus input signals anything", {
  seen <- character()
  for (case in read_corpus("delimited.json")) {
    input <- charToRaw(enc2utf8(case$input))
    d <- case$dialect
    dialect <- delimited_dialect(d$separator, quoting = d$quoting, has_header = d$has_header,
                                 skip_blank_lines = d$skip_blank_lines)
    plan <- c(plan_of(case), list(col_text(9), col_timestamp(1)))
    for (n in 0:length(input)) {
      cut <- input[seq_len(n)]
      seen <- c(seen, signalled({
        reader <- delimited_reader(cut, dialect, batch_rows = 2L)
        # A plan reaching past the header's width is the caller's error, not the data's: a
        # truncated header narrows what the plan may read.
        width <- length(header(reader))
        reach <- vapply(plan, function(column) column$source, 0)
        if (is.null(failure(reader))) {
          bind(reader, plan[!width | reach <= width])
          while (!is.null(read_batch(reader))) NULL
        }
      }))
    }
  }
  expect_identical(seen, character())
})

test_that("no truncated or corrupted workbook signals anything", {
  seen <- character()
  set.seed(20261101)
  for (file in c("basic.xlsx", "basic.ods", "numbers-data.numbers")) {
    bytes <- readBin(file.path(corpus_dir(), "workbook", file), "raw",
                     file.size(file.path(corpus_dir(), "workbook", file)))
    for (n in unique(c(0:64, round(seq(65, length(bytes), length.out = 60))))) {
      seen <- c(seen, signalled(read_every_sheet(bytes[seq_len(n)])))
    }
    for (i in 1:60) {
      broken <- bytes
      at <- sample(length(bytes), 8L)
      broken[at] <- as.raw(sample(0:255, 8L, replace = TRUE))
      seen <- c(seen, signalled(read_every_sheet(broken)))
    }
  }
  expect_identical(seen, character())
})
