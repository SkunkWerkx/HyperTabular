# The public surface beyond what the corpus pins: the verdict's shape, the spans' units, the
# plan and sheet declarations, the reader's life, and the version probe.

csv <- function(text) charToRaw(enc2utf8(text))

test_that("each plan column is a hypercast verdict, read by hypercast's own helpers", {
  t <- read_delimited(csv("id,amount\n1,2.5\n2,x\n3,\n"),
                      list(id = col_i32("id"), amount = col_f64("amount")))
  v <- t$columns$amount
  expect_s3_class(v, c("hypercast_verdict", "data.frame"), exact = TRUE)
  expect_identical(attr(v, "door"), "col_f64")
  expect_identical(names(v), c("value", "ok", "reason", "offset", "length", "raw"))
  expect_identical(hypercast::is_ok(v), c(TRUE, FALSE, FALSE))
  expect_identical(as.character(v$reason), c(NA, "malformed", "empty"))
  expect_identical(v$raw, c(NA, "x", ""))
  expect_identical(v$value, c(2.5, NA, NA))
  expect_identical(as.data.frame(t), data.frame(id = c(1, 2, 3), amount = c(2.5, NA, NA)))
  expect_identical(t$line, c(2, 3, 4))
  expect_identical(t$header, c("id", "amount"))
  expect_null(t$failure)
  expect_output(print(t), "3 rows, 2 columns")
})

test_that("a fault's span indexes raw in the units substr() counts", {
  # Characters for UTF-8: the core's byte spans (2 + 2 and 0 + 4) come out as 2 + 1 and 0 + 2.
  t <- read_delimited(csv("12\u00e9\n\u00e9\u00e9\n"), list(col_i32(1), col_date(1)),
                      delimited_dialect(has_header = FALSE))
  v <- t$columns[[1]]
  expect_identical(v$raw, c("12\u00e9", "\u00e9\u00e9"))
  expect_identical(Encoding(v$raw), c("UTF-8", "UTF-8"))
  expect_identical(c(v$offset[[1]], v$length[[1]]), c(2L, 1L))
  expect_identical(substr(v$raw[[1]], v$offset[[1]] + 1L, v$offset[[1]] + v$length[[1]]),
                   "\u00e9")
  v <- t$columns[[2]]
  expect_identical(c(v$offset[[2]], v$length[[2]]), c(0L, 2L))
  # Bytes, and raw marked "bytes", for text that is not UTF-8.
  t <- read_delimited(as.raw(c(0x31, 0xff, 0x0a)), list(col_i32(1)),
                      delimited_dialect(has_header = FALSE))
  v <- t$columns[[1]]
  expect_false(v$ok)
  expect_identical(Encoding(v$raw), "bytes")
  expect_identical(charToRaw(v$raw), as.raw(c(0x31, 0xff)))
  expect_identical(c(v$offset, v$length), c(1L, 1L))
  # A NUL: raw is NA, the span in bytes, and text refuses the cell at the NUL.
  t <- read_delimited(as.raw(c(0x61, 0x00, 0x62, 0x0a)), list(col_text(1), col_i32(1)),
                      delimited_dialect(has_header = FALSE))
  text <- t$columns[[1]]
  expect_identical(as.character(text$reason), "out_of_range")
  expect_identical(c(text$offset, text$length), c(1L, 1L))
  expect_identical(text$raw, NA_character_)
  expect_identical(as.character(t$columns[[2]]$reason), "malformed")
})

test_that("values come out typed as hypercast types them", {
  t <- read_delimited(csv(paste0("2026-07-01T12:30:00.5Z,2026-07-01,12:30:00,",
                                 "18446744073709551615,1.25,7/1/2026 13:05\n")),
                      list(ts = col_timestamp(1), d = col_date(2), tm = col_time(3),
                           u = col_u64(4), dec = col_decimal(5),
                           dt = col_datetime(6, "month_day_year")),
                      delimited_dialect(has_header = FALSE))
  c <- t$columns
  expect_s3_class(c$ts$value, "POSIXct")
  expect_identical(c$ts$nanos, 500000000L)
  expect_identical(c$d$value, as.Date("2026-07-01"))
  expect_s3_class(c$tm$value, "difftime")
  expect_identical(c$tm$nanos, 45000e9)
  expect_identical(c$u$value, "18446744073709551615")
  expect_identical(c$dec$value, "1.25")
  expect_identical(c$dt$value, "2026-07-01 13:05:00")
  expect_identical(c$dt$date, as.Date("2026-07-01"))
})

test_that("a numeric column with no format reads as hypercast's invariant notation does", {
  t <- read_delimited(csv("\"1,5\",\"1,234\"\n"), list(col_f64(1), col_f64(2)),
                      delimited_dialect(has_header = FALSE))
  expect_false(t$columns[[1]]$ok)
  expect_false(hypercast::cast_f64("1,5", hypercast::num_format())$ok)
  expect_identical(t$columns[[2]]$value, 1234)
})

test_that("number formats and declared parameters reach the door", {
  fr <- hypercast::num_format(",", "\u202f", "grouping")
  t <- read_delimited(csv("\"1\u202f234,5\"\n"), list(col_f64(1, fr), col_f64(1)),
                      delimited_dialect(has_header = FALSE))
  expect_identical(t$columns[[1]]$value, 1234.5)
  expect_false(t$columns[[2]]$ok)
  expect_error(col_unix(1, "minutes"), "precision must be one of")
  expect_error(col_excel_serial(1, "y1901"), "epoch must be one of")
  expect_error(col_datetime(1), "order")
  expect_error(col_i32(0), "whole-number position from 1")
  expect_error(col_i32(c("a", "b")), "one column name")
  expect_error(col_f64(1, "fr"), "hypercast_num_format")
  expect_output(print(col_date("when", "day_month_year")), "col_date")
})

test_that("a plan resolves names against the header, and is bound once", {
  reader <- delimited_reader(csv("a,b,a\n1,2,3\n"))
  expect_identical(header(reader), c("a", "b", "a"))
  expect_error(read_batch(reader), "no plan yet")
  expect_error(bind(reader, list(col_i32("c"))), "not in the header")
  expect_error(bind(reader, list(col_i32(4))), "past the 3 columns")
  expect_error(bind(reader, list(1)), "plan column 1 is not a column")
  expect_error(bind(reader, "x"), "plan must be a list")
  # The first of equal names; an unnamed column is named for its source's header name.
  bind(reader, list(first = col_i32("a"), col_i32(3)))
  expect_error(bind(reader, list(col_i32(1))), "bound once")
  batch <- read_batch(reader)
  expect_identical(names(batch$columns), c("first", "a"))
  expect_identical(batch$columns$first$value, 1)
  expect_identical(batch$columns[[2]]$value, 3)
  expect_null(read_batch(reader))
  expect_output(print(reader), "at the end")
  no_header <- delimited_reader(csv("1\n"), delimited_dialect(has_header = FALSE))
  expect_null(header(no_header))
  expect_error(bind(no_header, list(col_i32("a"))), "no header")
  expect_error(read_delimited(csv("1\n"), dialect = delimited_dialect(has_header = FALSE)),
               "plan must be given")
})

test_that("a plan given at open reads like one bound after", {
  input <- csv("a,b\n1,x\n2,y\n")
  plan <- list(col_text("b"), col_i32("a"))
  at_open <- delimited_reader(input, plan = plan)
  later <- bind(delimited_reader(input), plan)
  expect_identical(read_batch(at_open), read_batch(later))
})

test_that("with no plan, read_delimited reads every header column as text", {
  t <- read_delimited(csv("a,b\n1,x\n"))
  expect_identical(names(t$columns), c("a", "b"))
  expect_identical(t$columns$b$value, "x")
  t <- read_delimited(csv("1,x\n"), list(col_i32(1), b = col_text(2)),
                      delimited_dialect(has_header = FALSE))
  expect_identical(names(t$columns), c("V1", "b"))
})

test_that("an empty input has an empty header and no rows", {
  t <- read_delimited(raw(), list(x = col_i32(1)), delimited_dialect(has_header = FALSE))
  expect_identical(nrow(t$columns$x), 0L)
  expect_identical(t$line, numeric())
  expect_identical(header(delimited_reader(raw())), character())
})

test_that("a structural failure is a value, after the rows before it, and final", {
  reader <- delimited_reader(csv("a,b\n1,2\n3\n4,5\n"), plan = list(col_i32("a")))
  batch <- read_batch(reader)
  expect_identical(batch$columns[[1]]$value, 1)
  expect_null(read_batch(reader))
  f <- failure(reader)
  expect_s3_class(f, "hypertabular_failure")
  expect_identical(f$kind, "column_count")
  expect_identical(c(f$record, f$line, f$expected, f$found), c(2, 3, 2, 1))
  expect_null(read_batch(reader))
  expect_identical(failure(reader), f)
  expect_match(format(f), "Record 2 (line 3, byte 8) has 1 cells; the first record had 2.",
               fixed = TRUE)
  expect_output(print(f), "column_count")
  expect_output(print(reader), "failed")
  t <- read_delimited(csv("a\n\"open\n"), list(col_text(1)))
  expect_identical(t$failure$kind, "unclosed_quote")
  expect_identical(nrow(t$columns[[1]]), 0L)
  # A header that cannot be read is the failure, and the reader holds it from the start.
  reader <- delimited_reader(csv("\"a\n"))
  expect_null(header(reader))
  expect_identical(failure(reader)$kind, "unclosed_quote")
  expect_identical(read_delimited(csv("\"a\n"))$failure$kind, "unclosed_quote")
  # So is it when the plan names its sources: there is no header to find them in, and the
  # read ends at once on the failure rather than in an error.
  reader <- delimited_reader(csv("\"a\n"), plan = list(col_i32("a"), b = col_text("b")))
  expect_identical(failure(reader)$kind, "unclosed_quote")
  expect_null(read_batch(reader))
  expect_identical(failure(reader)$kind, "unclosed_quote")
  expect_error(bind(reader, list(col_i32(1))), "bound once")
  reader <- delimited_reader(csv("\"a\n"))
  bind(reader, list(col_i32("a")))
  expect_null(read_batch(reader))
  t <- read_delimited(csv("\"a\n"), list(col_i32("a")))
  expect_identical(t$failure$kind, "unclosed_quote")
  expect_identical(names(t$columns), "a")
})

test_that("the dialect is declared, and checked", {
  t <- read_delimited(csv("a;b\n\n1;\"x;y\"\n"), list(col_text(2)), delimited_dialect(";"))
  expect_identical(t$columns[[1]]$value, "x;y")
  expect_identical(t$line, 3)
  t <- read_delimited(csv("a\n\n1\n"), list(col_text(1)),
                      delimited_dialect(skip_blank_lines = FALSE))
  expect_identical(t$columns[[1]]$value, c(NA, "1"))
  expect_identical(as.character(t$columns[[1]]$reason), c("empty", NA))
  t <- read_delimited(csv("\"a\"\n"), list(col_text(1)),
                      delimited_dialect(quoting = FALSE, has_header = FALSE))
  expect_identical(t$columns[[1]]$value, "\"a\"")
  expect_error(delimited_dialect("\""), "separator")
  expect_error(delimited_dialect(";;"), "separator")
  expect_error(delimited_dialect(quoting = NA), "quoting must be TRUE or FALSE")
  expect_error(delimited_reader(csv("a"), "x"), "hypertabular_dialect")
  expect_error(delimited_reader(1), "raw vector, a file path or a connection")
  expect_error(delimited_reader(csv("a"), batch_rows = 0), "batch_rows")
  expect_error(delimited_reader(csv("a"), buffer_bytes = 1.5), "buffer_bytes")
  expect_output(print(delimited_dialect("\t")), "\\\\t")
})

test_that("a connection the reader opens it closes; one opened by the caller stays open", {
  path <- tempfile()
  on.exit(unlink(path))
  writeBin(csv("a\n1\n"), path)
  con <- file(path)
  reader <- delimited_reader(con, plan = list(col_i32(1)))
  expect_true(isOpen(con))
  while (!is.null(read_batch(reader))) NULL
  expect_error(isOpen(con), "invalid connection")
  con <- file(path, "rb")
  reader <- delimited_reader(con, plan = list(col_i32(1)))
  while (!is.null(read_batch(reader))) NULL
  expect_true(isOpen(con))
  close(con)
})

test_that("a batch holds at most batch_rows rows, and stays good after the reader moves on", {
  reader <- delimited_reader(csv("a\n1\n2\n3\n"), plan = list(col_i32(1)), batch_rows = 2)
  first <- read_batch(reader)
  second <- read_batch(reader)
  expect_identical(first$columns[[1]]$value, c(1, 2))
  expect_identical(second$columns[[1]]$value, 3)
  expect_identical(first$line, c(2, 3))
})

test_that("a workbook is opened, its sheets listed, and read by name or position", {
  path <- system.file("extdata", "basic.xlsx", package = "hypertabular")
  skip_if(path == "", "the example workbook is staged by tools/vendor.sh")
  book <- workbook(path)
  expect_identical(book$format, "xlsx")
  expect_identical(book$date_system, "y1900")
  expect_true("Data" %in% book$sheets$name)
  expect_output(print(book), "xlsx, y1900")
  by_name <- read_sheet(book, "Data", list(id = col_i32("id")))
  by_position <- read_sheet(path, match("Data", book$sheets$name), list(id = col_i32("id")))
  expect_identical(by_name, by_position)
  expect_gt(nrow(by_name$columns$id), 0L)
  expect_identical(workbook(readBin(path, "raw", file.size(path)))$sheets, book$sheets)
  expect_error(sheet_reader(book, "No such"), "no sheet named")
  expect_error(sheet_reader(book, 0), "no sheet 0")
  expect_error(sheet_reader(book, nrow(book$sheets) + 1), "and no sheet")
  expect_error(sheet_reader(book, TRUE), "sheet must be")
  expect_error(sheet_reader(book, has_header = NA), "has_header must be TRUE or FALSE")
  expect_error(sheet_reader(list()), "hypertabular_workbook")
  expect_error(workbook(1), "file path or a raw vector")
  expect_error(workbook(tempfile()), "cannot open file")
  reader <- sheet_reader(book, "Data")
  expect_output(print(reader), "sheet reader> unbound")
  expect_error(bind(reader, list(col_i32("no such header"))), "not in the header")
})

test_that("a workbook that cannot be read is a value, not an error", {
  book <- workbook(csv("not a workbook"))
  expect_identical(book$failure$kind, "not_a_zip")
  expect_identical(book$format, NA_character_)
  expect_identical(nrow(book$sheets), 0L)
  expect_output(print(book), "failed")
  expect_error(sheet_reader(book), "could not be read")
  t <- read_sheet(csv("not a workbook"), 1, list(x = col_i32(1)))
  expect_identical(t$failure$kind, "not_a_zip")
  expect_identical(nrow(t$columns$x), 0L)
})

test_that("the native core is this package's version", {
  expect_identical(hypertabular_native_version(), packageVersion("hypertabular"))
})
