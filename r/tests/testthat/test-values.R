# What values come out as: a table column and hypercast's own door agree on the same bytes;
# extrema, 2^53, decimal precision, temporal extremes, NUL, invalid UTF-8 and missingness;
# what the core refuses a plan for, and how a hypercast of another layout is named; and each
# declaration of a read doing what it says.

csv <- function(text) charToRaw(enc2utf8(text))

# A verdict without what only a table has (raw, origin) and with no door name, to compare a
# table column with hypercast's own door.
strip <- function(v) {
  v <- unclass(v)
  v$raw <- NULL
  v$origin <- NULL
  attr(v, "door") <- NULL
  v
}

# One cell per line of tab-separated, unquoted text, every case through `column` in one read.
table_cells <- function(cells, column) {
  line <- function(b) c(if (is.raw(b)) b else charToRaw(enc2utf8(b)), as.raw(0x0a))
  bytes <- unlist(lapply(cells, line), use.names = FALSE)
  read_delimited(bytes, list(column), delimited_dialect("\t", quoting = FALSE,
                                                        has_header = FALSE,
                                                        skip_blank_lines = FALSE))$columns[[1]]
}

test_that("standalone casts and table columns agree, NUL and invalid UTF-8 included", {
  text <- list(raw(), charToRaw("abc"), as.raw(c(0x61, 0x00, 0x62)), as.raw(c(0x31, 0xff)),
               charToRaw(enc2utf8("café")), charToRaw(" padded "), as.raw(0x00))
  expect_identical(strip(table_cells(text, col_text(1))), strip(hypercast::cast_text(text)))
  expect_identical(strip(table_cells(text, col_text(1, raw = TRUE))),
                   strip(hypercast::cast_text(text, raw = TRUE)))
  # cast_char() has no table door: a text column hands it the text the cell holds, so over
  # NUL-free, valid UTF-8 cells it judges a table's values as it judges the source's.
  chars <- c("a", "é", "ab", "", " ", "€", "😀", "x y")
  read <- table_cells(as.list(chars), col_text(1))$value
  read[is.na(read)] <- ""
  expect_identical(strip(hypercast::cast_char(read)), strip(hypercast::cast_char(chars)))
  # A NUL inside: ok, an NA value, and the bytes with raw = TRUE, on both surfaces.
  standalone <- hypercast::cast_text(list(as.raw(c(0x61, 0x00, 0x62))))
  expect_identical(c(standalone$ok, is.na(standalone$value)), c(TRUE, TRUE))
  numbers <- c("0", "-128", "127", "128", "-32768", "32767", "-2147483648", "2147483647",
               "2147483648", "-9223372036854775808", "9223372036854775807",
               "9223372036854775808", "18446744073709551615", "18446744073709551616", "1,234",
               "1,23,456", "x", "", " 1", "1e3", "-0")
  for (door in c("i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
                 "decimal")) {
    cast <- get(paste0("cast_", door), asNamespace("hypercast"))
    expect_identical(strip(table_cells(as.list(numbers), get(paste0("col_", door))(1))),
                     strip(cast(numbers, hypercast::num_format())), label = door)
  }
  reals <- c("9007199254740992", "9007199254740993", "-9007199254740993", "1.7976931348623157e308",
             "1.8e308", "4.9e-324", "0.1", "3.4028235e38", "3.5e38")
  expect_identical(strip(table_cells(as.list(reals), col_f64(1))),
                   strip(hypercast::cast_f64(reals, hypercast::num_format())))
  expect_identical(strip(table_cells(as.list(reals), col_f32(1))),
                   strip(hypercast::cast_f32(reals, hypercast::num_format())))
  decimals <- c("79228162514264337593543950335", "79228162514264337593543950336",
                "0.0000000000000000000000000001", "-0.5", "1.00")
  expect_identical(strip(table_cells(as.list(decimals), col_decimal(1))),
                   strip(hypercast::cast_decimal(decimals, hypercast::num_format())))
  instants <- c("0001-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z",
                "1969-12-31T23:59:59.5Z", "2026-07-01T12:30:00+05:30", "2026-02-30T00:00:00Z")
  expect_identical(strip(table_cells(as.list(instants), col_timestamp(1))),
                   strip(hypercast::cast_timestamp(instants)))
  dates <- c("0001-01-01", "9999-12-31", "2024-02-29", "2023-02-29", "20260107")
  expect_identical(strip(table_cells(as.list(dates), col_date(1))),
                   strip(hypercast::cast_date(dates)))
  options <- hypercast::date_options("compact_date")
  expect_identical(strip(table_cells(as.list(dates), col_date(1, options = options))),
                   strip(hypercast::cast_date(dates, options = options)))
  civil <- c("1/7/2026 3:04 PM", "12/31/9999 23:59:59.999999999", "2/30/2026 0:00")
  expect_identical(strip(table_cells(as.list(civil), col_datetime(1, "month_day_year"))),
                   strip(hypercast::cast_datetime(civil, "month_day_year")))
  times <- c("00:00:00", "23:59:59.999999999", "24:00:00", "15:04:05.5")
  expect_identical(strip(table_cells(as.list(times), col_time(1))),
                   strip(hypercast::cast_time(times)))
  durations <- c("PT0S", "PT1H30M", "-PT1.5S", "P1D")
  expect_identical(strip(table_cells(as.list(durations), col_duration(1))),
                   strip(hypercast::cast_duration(durations)))
  units <- c("1700000000123", "-1", "9223372036854775807")
  expect_identical(strip(table_cells(as.list(units), col_unix(1, "milliseconds"))),
                   strip(hypercast::cast_unix(units, "milliseconds")))
  serials <- c("45292.75", "0", "-1", "2958466")
  expect_identical(strip(table_cells(as.list(serials), col_excel_serial(1, "y1900"))),
                   strip(hypercast::cast_excel_serial(serials, "y1900")))
  uuids <- c("6ba7b810-9dad-11d1-80b4-00c04fd430c8", "6BA7B8109DAD11D180B400C04FD430C8", "x")
  expect_identical(strip(table_cells(as.list(uuids), col_uuid(1))),
                   strip(hypercast::cast_uuid(uuids)))
  bools <- c("true", "FALSE", "yes", "0", "maybe")
  expect_identical(strip(table_cells(as.list(bools), col_bool(1))),
                   strip(hypercast::cast_bool(bools)))
})

test_that("a civil date-time is a wall clock, and an instant a moment in UTC", {
  t <- read_delimited(csv("2026-07-01T12:30:00.5Z,7/1/2026 13:05\n"),
                      list(at = col_timestamp(1), wall = col_datetime(2, "month_day_year")),
                      delimited_dialect(has_header = FALSE))
  expect_s3_class(t$columns$at$value, "POSIXct")
  expect_identical(attr(t$columns$at$value, "tzone"), "UTC")
  expect_identical(t$columns$at$nanos, 500000000L)
  expect_type(t$columns$wall$value, "character")
  expect_identical(t$columns$wall$value, "2026-07-01 13:05:00")
  expect_s3_class(t$columns$wall$date, "Date")
})

test_that("missingness is told apart through ok, reason and origin", {
  t <- read_delimited(csv("a,b\nNA,1\n,2\nx,3\n4\n"), list(a = col_i32("a")), fill = TRUE,
                      na = "NA")
  v <- t$columns$a
  expect_identical(v$ok, c(FALSE, FALSE, FALSE, TRUE))
  expect_identical(as.character(v$reason), c("empty", "empty", "malformed", NA))
  expect_identical(as.character(v$origin), c("token", "cell", "cell", "cell"))
  t <- read_delimited(csv("a,b\n1\n"), list(b = col_i32("b")), fill = TRUE)
  expect_identical(as.character(t$columns$b$origin), "padded")
  expect_identical(as.character(t$columns$b$reason), "empty")
  # Only what went wrong is a problem: neither the token, the blank nor the padding.
  t <- read_delimited(csv("a,b\nNA,1\n,2\nx,3\n4\n"), list(a = col_i32("a"), b = col_i32("b")),
                      fill = TRUE, na = "NA")
  p <- problems(t)
  expect_identical(p$row, 3L)
  expect_identical(p$column, "a")
  # A column's own na replaces the read's; character(0) declares none.
  t <- read_delimited(csv("a\nNA\n-\n"), list(own = col_text("a", na = "-"),
                                              none = col_text("a", na = character()),
                                              inherited = col_text("a")), na = "NA")
  expect_identical(as.character(t$columns$own$origin), c("cell", "token"))
  expect_identical(t$columns$none$value, c("NA", "-"))
  expect_identical(as.character(t$columns$inherited$origin), c("token", "cell"))
})

test_that("an unknown detail code reads as unknown with its number kept", {
  levels <- c(names(hypercast::fault_details), "unknown")
  f <- hypertabular:::detail_factor(c(2L, 999L, NA))
  expect_identical(levels(f), levels)
  expect_identical(as.character(f), c("unexpected_character", "unknown", NA))
  cols <- list(value = c(NA_real_, NA_real_), ok = c(FALSE, FALSE), reason = c(2L, 2L),
               offset = c(0L, 0L), length = c(1L, 1L), raw = c("x", "y"), detail = c(2L, 999L),
               origin = c(0L, 7L))
  v <- hypertabular:::as_verdict(cols, col_f64(1))
  expect_identical(as.character(v$detail), c("unexpected_character", "unknown"))
  expect_identical(v$detail_code, c(2L, 999L))
  expect_identical(as.character(v$origin), c("cell", "unknown"))
})

# A packed format with its size word, or a word at `at` (from 0), set to `value`.
doctored <- function(packed, at, value) {
  packed[at + 1:4] <- hypertabular:::u32_le(value)
  packed
}

test_that("a packed format of an unknown size is an error naming both versions", {
  format <- hypercast::num_format()
  bigger <- format
  bigger$packed <- c(doctored(format$packed, 0, 100), raw(8))
  versions <- sprintf("hypercast %s .*hypertabular %s", packageVersion("hypercast"),
                      packageVersion("hypertabular"))
  expect_error(read_delimited(csv("a\n1\n"), list(col_i32("a", bigger))), versions)
  smaller <- format
  smaller$packed <- doctored(format$packed, 0, 29)[1:29]
  expect_error(read_delimited(csv("a\n1\n"), list(col_i32("a", smaller))), "100|29")
  expect_error(read_delimited(csv("a\n1\n"), list(col_i32("a", smaller))), versions)
  options <- hypercast::date_options("compact_date")
  options$packed <- doctored(options$packed, 0, 16)
  expect_error(read_delimited(csv("a\n1\n"), list(col_date("a", options = options))), versions)
  # The negative control: the same formats at their own size read.
  expect_identical(read_delimited(csv("a\n1\n"), list(col_i32("a", format)))$columns[[1]]$value,
                   1)
})

stale <- function(flags) {
  format <- hypercast::num_format()
  format$packed <- doctored(format$packed, 4, flags)
  format
}

test_that("a stale grouping word (351, 863) is refused at bind, at every numeric door", {
  for (flags in c(351, 863)) {
    for (door in hypertabular:::numeric_doors) {
      column <- get(paste0("col_", door))(1, stale(flags))
      expect_error(read_delimited(csv("a\n1\n"), list(column)),
                   "plan column 1 cannot be honoured: the core refuses what it declares",
                   label = paste(door, flags))
      # Refused before a sheet is read as well.
      book <- workbook(testthat::test_path("corpus", "workbook", "grouping.xlsx"))
      expect_error(sheet_reader(book, 1, list(column)), "plan column 1 cannot be honoured")
      close(book)
    }
    expect_error(suggest_plan(csv("a\n1\n"), format = stale(flags)),
                 "^format cannot be honoured")
  }
  # The negative control: 95 and 607 are policies, and read.
  for (flags in c(95, 607)) {
    expect_null(read_delimited(csv("a\n1\n"), list(col_i32("a", stale(flags))))$failure)
  }
})

test_that("the 1.0 exports read every format Western, 351, 607 and 863 alike", {
  dialect <- as.raw(c(0x2c, 1, 1, 0))
  spec <- function(ordinal, flags) {
    c(hypertabular:::u32_le(ordinal), hypertabular:::u32_le(4), hypertabular:::u32_le(0),
      stale(flags)$packed)
  }
  input <- csv("\"1,234,567\",\"1,23,456\"\n")
  for (flags in c(351, 607, 863)) {
    reasons <- .Call(hypertabular:::C_one_dot_zero_delimited, input, dialect,
                     c(spec(0, flags), spec(1, flags)))
    expect_identical(reasons, list(0L, 2L), label = flags)
  }
  # The sheet exports: grouping.xlsx's columns are Indian, Western and both, as text.
  book <- workbook(testthat::test_path("corpus", "workbook", "grouping.xlsx"))
  on.exit(close(book), add = TRUE)
  for (flags in c(351, 607, 863)) {
    reasons <- .Call(hypertabular:::C_one_dot_zero_sheet, book$state$ptr, book$parts[[1]],
                     book$indexes[[1]], c(spec(0, flags), spec(1, flags)))
    # 1,23,456 and 12,34,567 refused, 1234 read; 1,234,567, 1,234 and 1234 read.
    expect_identical(reasons, list(c(2L, 2L, 0L), c(0L, 0L, 0L)), label = flags)
  }
})

test_that("an Indian column beside a Western one reads each as declared", {
  indian <- hypercast::num_format_with_grouping(hypercast::num_format(), "indian")
  input <- csv("i,w\n\"1,23,456\",\"1,234,567\"\n\"1,234,567\",\"1,23,456\"\n")
  t <- read_delimited(input, list(i = col_i32("i", indian), w = col_i32("w")))
  expect_identical(t$columns$i$value, c(123456, NA))
  expect_identical(t$columns$w$value, c(1234567, NA))
  expect_identical(as.character(t$columns$i$reason), c(NA, "malformed"))
  expect_identical(as.character(t$columns$w$reason), c(NA, "malformed"))
})

test_that("a named plan refuses a duplicate", {
  expect_error(read_delimited(charToRaw("id,id\n1,2\n"), plan = list(x = col_i32("id"))),
               "columns 1 and 2")
  # The negative control: a position reads either.
  expect_identical(read_delimited(charToRaw("id,id\n1,2\n"),
                                  plan = list(x = col_i32(2)))$columns$x$value, 2)
})

test_that("repeated output names are made unique, and the originals kept", {
  t <- read_delimited(csv("a,b\n1,2\n"), list(col_i32("a"), col_text("a"), b = col_i32("b")))
  expect_identical(names(t$columns), c("a", "a_1", "b"))
  expect_identical(t$original_names, c("a", "a", "b"))
  expect_identical(names(as.data.frame(t)), c("a", "a_1", "b"))
  expect_identical(t$header, c("a", "b"))
})

test_that("ragged, skip, range, na and encoding each read as declared", {
  # skip passes over lines whatever they hold, header_row discards records after them.
  t <- read_delimited(csv("junk \"\nmore\nskipped,record\nid,n\n1,2\n"), skip = 2, header_row = 1)
  expect_identical(t$header, c("id", "n"))
  expect_identical(t$line, 5)
  # fill pads a short record; reject is a failure after the rows before it.
  expect_identical(read_delimited(csv("a,b\n1\n"), list(col_text("b")), fill = TRUE)$failure,
                   NULL)
  expect_identical(read_delimited(csv("a,b\n1,2\n3\n"), list(col_text("b")))$failure$kind,
                   "column_count")
  # long_rows: truncate cuts, report cuts and lists, reject fails.
  long <- csv("a\n1\n2,3\n")
  expect_identical(read_delimited(long, list(col_i32("a")))$failure$kind, "column_count")
  t <- read_delimited(long, list(col_i32("a")), long_rows = "truncate")
  expect_identical(c(t$columns$a$value, nrow(problems(t))), c(1, 2, 0))
  t <- read_delimited(long, list(col_i32("a")), long_rows = "report")
  p <- problems(t)
  expect_identical(c(as.character(p$kind), p$expected, p$found, p$line), c("row", "1", "2", "3"))
  # encoding: decoded ahead of the read; a unit with no character ends it after its rows.
  t <- read_delimited(as.raw(c(0x61, 0x0a, 0x80, 0x0a)), encoding = "windows-1252")
  expect_identical(t$columns[[1]]$value, "€")
  t <- read_delimited(as.raw(c(0xff, 0xfe, 0x61, 0, 0x0a, 0, 0xe9, 0, 0x0a, 0)),
                      encoding = "utf-16le")
  expect_identical(t$columns[[1]]$value, "é")
  t <- read_delimited(as.raw(c(0xff, 0xfe, 0x61, 0, 0x0a, 0, 0x31, 0, 0x0a, 0, 0, 0xdc, 0x0a, 0)),
                      encoding = "utf-16le")
  expect_identical(c(t$columns[[1]]$value, t$failure$kind, t$failure$byte),
                   c("1", "encoding", "10"))
  # range and skip in a sheet, and the source column problems() names.
  book <- workbook(testthat::test_path("corpus", "workbook", "ranges.xlsx"))
  on.exit(close(book), add = TRUE)
  t <- read_sheet(book, 1, list(col_i32(1)), range = "B4:E12", na = c("NA", "-"))
  p <- problems(t)
  expect_true(all(p$source_column == 2))
  expect_identical(unique(p$sheet), book$sheets$name[[1]])
  expect_error(read_sheet(book, 1, range = "not a range"), "range must be")
})

test_that("a suggestion's param is the door's parameter, as Rust's report gives it", {
  # Rust's values, from its report on the same samples: an ambiguous date's param is the first
  # candidate's order (2, month_day_year), never the core's bitmask of both (12); a number's is
  # 0 whatever its notation, which `candidates` or `format` carries instead.
  s <- suggest_plan(csv(paste0("when,a,b,c\n01/02/2026,\"1,234\",\"0,125\",\"1,234,567\"\n",
                               "03/04/2026,\"2,345\",\"0,5\",\"2,345,678\"\n")))
  r <- s$report
  expect_identical(r$door, c("date_ordered", "i32", "f64", "i32"))
  expect_identical(r$status, c("ambiguous", "ambiguous", "resolved", "resolved"))
  expect_identical(r$param, c(2, 0, 0, 0))
  expect_identical(r$candidates[[1]], list("month_day_year", "day_month_year"))
  expect_length(r$candidates[[2]], 2)
  expect_identical(r$format[[3]]$packed, hypercast::num_format(",", ".")$packed)
  expect_identical(r$format[[4]]$packed, hypercast::num_format()$packed)
  # A date order the caller declares resolves it, and param is that order.
  declared <- suggest_plan(csv("when\n01/02/2026\n03/04/2026\n"), date_order = "day_month_year")
  expect_identical(declared$report$param, 3)
  expect_identical(declared$report$status, "resolved")
})

test_that("suggest_plan prints a plan that reads back", {
  input <- csv("id,when,amount,note\n1,2026-01-07,\"1,234.5\",a\n2,2026-01-08,12,b\n")
  s <- suggest_plan(input)
  printed <- capture.output(print(s))
  plan <- eval(parse(text = printed))
  expect_identical(plan, s$plan)
  t <- read_delimited(input, plan)
  expect_identical(t$columns$amount$value, c(1234.5, 12))
  expect_identical(s$report$status, rep("resolved", 4))
  # An explicit 0 rows means 1.
  expect_identical(suggest_plan(input, sample_rows = 0)$rows, 1)
  expect_true(suggest_plan(input, sample_rows = 1)$truncated)
  expect_false(suggest_plan(input, sample_rows = 2)$truncated)
  # A failure among the sampled rows is the suggestion's; just past a whole sample, more.
  broken <- csv("a,b\n1,2\n3\n")
  expect_identical(suggest_plan(broken)$failure$kind, "column_count")
  expect_null(suggest_plan(broken)$plan)
  expect_true(suggest_plan(broken, sample_rows = 1)$truncated)
})

# readBin, made to raise as a failing disk would from its `fail`-th call on: R's own file
# connections stop short in silence instead, so the failure is staged where R would raise it.
failing_reads <- function(fail) {
  counter <- new.env()
  counter$calls <- 0
  tracer <- bquote({
    assign("calls", get("calls", envir = .(counter)) + 1, envir = .(counter))
    if (get("calls", envir = .(counter)) >= .(fail)) stop("the disk went away")
  })
  suppressMessages(trace("readBin", tracer = tracer, print = FALSE,
                         where = asNamespace("hypertabular")))
}

test_that("past a whole sample a connection's error says there is more; inside it, it's raised", {
  path <- tempfile(fileext = ".csv")
  on.exit(unlink(path), add = TRUE)
  writeLines(c("a", as.character(seq_len(20000))), path)
  on.exit(suppressMessages(untrace("readBin", where = asNamespace("hypertabular"))), add = TRUE)
  # The first read, of 64 bytes, holds the header and 23 whole rows, which the first batch
  # delivers; the second read is the one that fails, just past a sample of 23.
  failing_reads(2)
  s <- suggest_plan(path, sample_rows = 23, buffer_bytes = 64)
  expect_true(s$truncated)
  expect_identical(s$rows, 23)
  # One row more and the same failure is inside the sample.
  failing_reads(2)
  expect_error(suggest_plan(path, sample_rows = 24, buffer_bytes = 64), "the disk went away",
               class = "hypertabular_io_error")
  failing_reads(3)
  expect_error(suggest_plan(path, sample_rows = 1000, buffer_bytes = 64), "the disk went away",
               class = "hypertabular_io_error")
  # A read raises it whatever the rows before it: only a sample has a whole to be past.
  failing_reads(2)
  expect_error(read_delimited(path, buffer_bytes = 64), "the disk went away")
  # Any other condition propagates, past a whole sample too: a condition of the caller's own,
  # as an interrupt would (which testthat would take as its own).
  counter <- new.env()
  counter$calls <- 0
  suppressMessages(trace("readBin", print = FALSE, where = asNamespace("hypertabular"),
                         tracer = bquote({
                           assign("calls", get("calls", envir = .(counter)) + 1, envir = .(counter))
                           if (get("calls", envir = .(counter)) >= 2) {
                             stop(structure(class = c("caller_signal", "condition"),
                                            list(message = "the caller's", call = NULL)))
                           }
                         })))
  said <- tryCatch(suggest_plan(path, sample_rows = 23, buffer_bytes = 64),
                   caller_signal = function(c) conditionMessage(c))
  expect_identical(said, "the caller's")
  suppressMessages(untrace("readBin", where = asNamespace("hypertabular")))
  # A warning the caller made an error (options(warn = 2)) is the caller's too, past a whole
  # sample: raised, never `truncated`. Without warn = 2 the same warning only warns.
  warns <- function() {
    counter$calls <- 0
    suppressMessages(trace("readBin", print = FALSE, where = asNamespace("hypertabular"),
                           tracer = bquote({
                             assign("calls", get("calls", envir = .(counter)) + 1,
                                    envir = .(counter))
                             if (get("calls", envir = .(counter)) == 2) warning("a slow disk")
                           })))
  }
  warns()
  strict <- function(expr) {
    old <- options(warn = 2)
    on.exit(options(old))
    expr
  }
  expect_error(strict(suggest_plan(path, sample_rows = 23, buffer_bytes = 64)), "a slow disk")
  warns()
  expect_warning(s <- suggest_plan(path, sample_rows = 23, buffer_bytes = 64), "a slow disk")
  expect_identical(s$rows, 23)
  # A connection closed under the reader is the caller's: an error that says so, raised past a
  # whole sample too.
  con <- file(path, "rb")
  counter$calls <- 0
  suppressMessages(trace("readBin", print = FALSE, where = asNamespace("hypertabular"),
                         tracer = bquote({
                           assign("calls", get("calls", envir = .(counter)) + 1,
                                  envir = .(counter))
                           if (get("calls", envir = .(counter)) == 2) close(.(con))
                         })))
  expect_error(suggest_plan(con, sample_rows = 23, buffer_bytes = 64),
               "the reader's connection was closed while the reader was reading it")
  suppressMessages(untrace("readBin", where = asNamespace("hypertabular")))
  # The negative control: untraced, the same sample reads its rows.
  expect_identical(suggest_plan(path, sample_rows = 1000, buffer_bytes = 64)$rows, 1000)
  # A connection the caller opened in text mode can't be read as bytes: refused before any read.
  text <- file(path, "r")
  on.exit(close(text), add = TRUE)
  expect_error(delimited_reader(text), "a connection must be opened in binary mode")
  # The two text idioms are pointed at what reads them.
  expect_error(delimited_reader(stdin()), "for standard input, file(\"stdin\")", fixed = TRUE)
  lines <- textConnection("a\n1")
  on.exit(close(lines), add = TRUE)
  expect_error(delimited_reader(lines), "a raw vector (charToRaw())", fixed = TRUE)
  expect_identical(read_delimited(charToRaw("a\n1\n"), list(col_i32("a")))$columns[[1]]$value, 1)
})

test_that("sizes are the core's to refuse, before anything is allocated", {
  input <- csv("a\n1\n")
  expect_error(read_delimited(input, batch_rows = 0), "batch_rows must be at least 1")
  expect_error(read_delimited(input, buffer_bytes = 0), "buffer_bytes must be at least 1")
  # A sheet's 0 is the core's to refuse too, read or sampled: never a sample of no rows.
  book <- workbook(testthat::test_path("corpus", "workbook", "basic.xlsx"))
  expect_error(sheet_reader(book, 1, batch_rows = 0), "batch_rows must be at least 1")
  expect_error(suggest_plan(book, batch_rows = 0), "batch_rows must be at least 1")
  expect_identical(suggest_plan(book, batch_rows = 1)$rows, 1)
  # Past what any address space holds: refused in the core's words, so nothing is asked for.
  expect_error(read_delimited(input, batch_rows = 2^62), "past the limit of")
  expect_error(delimited_reader(input, plan = list(col_i32(1)), batch_rows = 2^56),
               "past the limit of")
  # One row past the limit, a plan reaching the last position a u32 ordinal names: asked of the
  # core's arithmetic alone, which allocates nothing whatever it answers.
  far <- list(col_i32(2^32))
  packed <- hypertabular:::pack_specs(far, hypertabular:::resolve_plan(far, NULL)$ordinals)
  said <- "a row of 4294967296 columns asks for 34359738392 bytes, past the limit of 4294967296"
  expect_error(hypertabular:::check_sizes(packed, 1, 1, 0L), said)
  expect_error(hypertabular:::check_sizes(packed, 1, 1, hypertabular:::batch_sheet),
               "a row of 4294967296 columns asks for [0-9]+ bytes, past the limit of 4294967296")
  expect_error(col_i32(2^32 + 1), "one whole-number position from 1")
  expect_error(sheet_reader(workbook(testthat::test_path("corpus", "workbook", "basic.xlsx")),
                            batch_rows = 2^62), "past the limit of")
  expect_error(read_delimited(input, batch_rows = -1), "whole number")
  expect_error(read_delimited(input, batch_rows = 1.5), "whole number")
  expect_error(read_delimited(input, batch_rows = 2^64), "whole number")
  # A sample past a batch's limit is only a sample.
  expect_identical(suggest_plan(input, sample_rows = 2^62)$rows, 1)
})

test_that("a header's edge cases read as the source holds them", {
  expect_identical(header(delimited_reader(raw())), character())
  expect_identical(header(delimited_reader(csv("a,a,\n"))), c("a", "a", ""))
  expect_identical(header(delimited_reader(as.raw(c(0x61, 0, 0x2c, 0x62, 0x0a)))), c(NA, "b"))
  t <- read_delimited(csv("a,b\n"))
  expect_identical(c(length(t$line), names(t$columns)), c("0", "a", "b"))
})
