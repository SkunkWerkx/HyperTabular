# A column that declares hypercast's constraints: what the corpus does not reach of the
# package's surface. An R column is a list anyone can edit, so the declaration's door is checked
# when the column is made and the declaration judged again, from its fields, when the plan is
# bound. R has no per-cell getter to refuse: a constrained column's whole verdict is hypercast's
# checked verdict, whose shape says so.

headerless <- delimited_dialect(has_header = FALSE)

read <- function(text, ...) {
  read_delimited(charToRaw(enc2utf8(text)), plan = list(...), dialect = headerless)
}

at_most <- function(maximum) hypercast::hypercast_constraints("i32", maximum = maximum)

test_that("a checked column reads a violation, keeping the value as cast", {
  t <- read("11\n7\nx\n", v = col_i32(1, constraints = at_most(10L)))
  v <- t$columns$v
  expect_s3_class(v, "hypercast_checked_verdict")
  expect_identical(v$value, c(11, 7, NA))
  expect_identical(v$ok, c(FALSE, TRUE, FALSE))
  expect_identical(v$keyword, c("maximum", NA, NA))
  expect_identical(as.character(v$reason), c(NA, NA, "malformed"))
  expect_identical(v$adjusted, c(NA, FALSE, NA))
  expect_identical(c(v$offset[[1]], v$length[[1]], v$raw[[1]]), c("0", "2", "11"))
  # hypercast's own helpers read it as they read a checked door's verdict.
  expect_identical(hypercast::violations(v)$row, 1L)
  expect_identical(hypercast::faults(v)$row, 3L)
  expect_identical(hypercast::is_ok(v), v$ok)
})

test_that("the column agrees with hypercast's own checked door on the same text", {
  cells <- c("12.37", "12.35", "-0", "x", "", "1e400")
  cash <- hypercast::hypercast_constraints("decimal", multiple_of = "0.05", maximum = "100")
  # Each cell quoted, so the empty one is a cell rather than a blank line the read skips.
  quoted <- paste0("\"", cells, "\"\n", collapse = "")
  t <- read(quoted, v = col_decimal(1, constraints = cash))
  table <- unclass(t$columns$v)
  table$raw <- NULL
  table$origin <- NULL
  attr(table, "door") <- NULL
  door <- unclass(hypercast::cast_checked(cells, hypercast::num_format(), cash))
  attr(door, "door") <- NULL
  expect_identical(table, door)
})

test_that("a rounded value is flagged adjusted", {
  floor <- hypercast::hypercast_constraints("decimal", multiple_of = "0.5", rounding = "floor")
  v <- read("1.3\n1.5\n", v = col_decimal(1, constraints = floor))$columns$v
  expect_identical(v$value, c("1", "1.5"))
  expect_identical(v$adjusted, c(TRUE, FALSE))
})

test_that("a text enum match delivers the declared entry", {
  sex <- hypercast::hypercast_constraints("text", enum = c("Female", "Male"))
  v <- read("MALE\nother\n", v = col_text(1, constraints = sex))$columns$v
  expect_identical(v$value, c("Male", "other"))
  expect_identical(v$enum_index, c(2L, NA))
  expect_identical(v$keyword, c(NA, "enum"))
  # With raw = TRUE, the bytes hypercast packed for the entry: what the core matched.
  v <- read("MALE\nother\n", v = col_text(1, raw = TRUE, constraints = sex))$columns$v
  expect_identical(v$value, list(charToRaw("Male"), charToRaw("other")))
})

test_that("an unconstrained column keeps its plain verdict, faults and all", {
  v <- read("5\n\"\"\nx\n", v = col_i32(1))$columns$v
  expect_s3_class(v, "hypercast_verdict")
  expect_false(inherits(v, "hypercast_checked_verdict"))
  expect_null(v$keyword)
  expect_identical(as.character(v$reason), c(NA, "empty", "malformed"))
  # A read of nothing has the shape a read of rows has.
  none <- read("", v = col_i32(1, constraints = at_most(10L)))$columns$v
  expect_s3_class(none, "hypercast_checked_verdict")
  one <- read("1\n", v = col_i32(1, constraints = at_most(10L)))$columns$v
  expect_identical(names(none), names(one))
})

test_that("a mismatched door is refused when the column is made and when the plan is bound", {
  expect_error(col_u8(1, constraints = at_most(10L)),
               "constraints for \"i32\" were given to a col_u8() column", fixed = TRUE)
  expect_error(col_i32(1, constraints = list(maximum = 10L)),
               "constraints must be a hypercast_constraints", fixed = TRUE)
  expect_error(col_i32(1, constraints = hypercast::hypercast_constraints("text")),
               "constraints for \"text\" were given to a col_i32() column", fixed = TRUE)
  # A column edited after its factory is judged again at bind: another door, a declaration
  # edited into one hypercast refuses.
  moved <- col_i32(1, constraints = at_most(10L))
  moved$door <- "i64"
  expect_error(read("1\n", v = moved),
               paste("plan column 1 cannot be honoured:",
                     "constraints for \"i32\" were given to a col_i64() column"),
               fixed = TRUE)
  edited <- col_i32(1, constraints = at_most(10L))
  edited$constraints$maximum <- 10.5
  expect_error(read("1\n", v = edited), "plan column 1 cannot be honoured: maximum must be NULL")
  # A uuid column has no constraints argument; one smuggled in is refused the same way.
  uuid <- col_uuid(1)
  uuid$constraints <- hypercast::hypercast_constraints("text")
  expect_error(read("1\n", v = uuid), "given to a col_uuid() column", fixed = TRUE)
  # The control: the same edit, back to what it was declared as, reads.
  edited$constraints$maximum <- 10L
  expect_identical(read("1\n", v = edited)$columns$v$ok, TRUE)
})

test_that("problems() lists a violation with its keyword and no reason", {
  p <- problems(read("11\nx\n", v = col_i32(1, constraints = at_most(10L))))
  expect_identical(p$keyword, c("maximum", NA))
  expect_identical(as.character(p$reason), c(NA, "malformed"))
  expect_identical(as.character(p$kind), c("cell", "cell"))
  expect_identical(c(p$offset[[1]], p$length[[1]], p$raw[[1]]), c("0", "2", "11"))
  # An unconstrained column's problems carry the column, with no keyword.
  expect_identical(problems(read("x\n", v = col_i32(1)))$keyword, NA_character_)
})

test_that("checks are sized like verdicts, across every batch", {
  r <- delimited_reader(charToRaw(strrep("11\n", 37)), headerless,
                        list(v = col_i32(1, constraints = at_most(10L))), batch_rows = 5)
  keywords <- character()
  while (!is.null(b <- read_batch(r))) keywords <- c(keywords, b$columns$v$keyword)
  expect_identical(keywords, rep("maximum", 37))
})

test_that("a constrained sheet column is judged", {
  path <- testthat::test_path("corpus", "workbook", "constraints.xlsx")
  v <- read_sheet(path, plan = list(v = col_i32(1, constraints = at_most(10L))))$columns$v
  expect_identical(v$value[[1]], 0)
  expect_true(any(v$keyword %in% "maximum" & v$value %in% 42))
})

test_that("a column prints as the call that makes it, declaration and all", {
  expect_output(print(col_i32("n", constraints = at_most(10L))),
                paste0("col_i32(\"n\", constraints = ",
                       "hypercast::hypercast_constraints(\"i32\", maximum = 10L))"),
                fixed = TRUE)
})
