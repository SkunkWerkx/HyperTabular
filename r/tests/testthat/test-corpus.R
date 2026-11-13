# Replays the two conformance corpus files at the repository root, the same files the Rust
# core and every other binding replay, through the public API, every key they hold read and a
# key this replay doesn't know a failure.
#
# corpus/delimited.json: every case from a raw vector, from connections read through buffers
# too small for a record, from a file and from an unopened connection, in batches of one row,
# two rows and many; with the plan bound after the header, by position and by header name;
# and with every buffer the core works in starting at one element. How the input is cut up
# must not change the answer: not the rows, not problems(), not the failure. A case's suggest
# is held to what suggest_plan() says of it, from memory and from a file.
#
# corpus/workbook.json and the packages it names under corpus/workbook/: every sheet of every
# package read whole, through read_sheet() with the sheet opened by its position; and a batch
# of seven rows at a time through sheet_reader() and bind(), the sheet opened by name where
# its name is unique and the plan naming its sources by header name where the header has them
# once, with every buffer starting at one element, so the grow-and-keep path runs mid-part
# throughout. The two encrypted files are refused as they open.
#
# The corpus pins fault spans as byte offsets into the cell's text, and rows and source columns
# from 0; this binding presents spans in the units substr() counts `raw` in, characters for
# UTF-8 text, and rows and columns from 1, so each pinned one is re-expressed before comparing.

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
  exact(cases)
}

# A parsed corpus whose `$` matches a key exactly: a list's own `$` would read `skip` from a
# dialect that has only `skip_blank_lines`, or `raw` from a cell that has only `raw_hex`.
exact <- function(value) {
  if (!is.list(value)) return(value)
  value[] <- lapply(value, exact)
  class(value) <- "corpus_object"
  value
}

`$.corpus_object` <- function(x, name) .subset2(x, name, exact = TRUE)

# Every key a replay here reads, by where it is: one that isn't here fails unknown_keys(), so a
# key added to the corpus can't be passed over. `note` is the one no replay reads.
known_keys <- list(
  cell = c("expect", "value", "text", "seconds", "nanos", "year", "month", "day",
           "nanos_of_day", "magnitude", "scale", "negative", "fault", "raw", "raw_hex", "detail",
           "origin", "keyword", "adjusted", "enum_index", "bits"),
  problem = c("row", "line", "plan_column", "source_column", "reason", "fault", "detail",
              "origin", "expected", "found", "keyword"),
  failure = c("kind", "record", "line", "byte", "expected", "found"),
  plan = c("door", "ordinal", "precision", "order", "epoch", "format", "tokens", "options",
           "constraints"),
  # A plan entry's constraints, by the OpenAPI names hypercast's corpus gives them.
  constraints = c("minimum", "exclusiveMinimum", "maximum", "exclusiveMaximum", "multipleOf",
                  "rounding", "minLength", "maxLength", "trim", "caseSensitive", "enum"),
  format = c("decimal_sep", "group_sep", "flags", "currency"),
  date = c("flags", "pivot", "months"),
  delimited = c("name", "input", "input_hex", "dialect", "plan", "header", "rows", "problems",
                "failure", "suggest"),
  dialect = c("separator", "quoting", "has_header", "skip_blank_lines", "skip", "header_row",
              "short_rows", "long_rows", "tokens", "encoding"),
  suggest = c("columns", "truncated", "sample_rows", "format"),
  suggested = c("door", "param", "status", "matched", "sampled"),
  workbook = c("name", "file", "format", "epoch", "sheets", "sheet", "options", "plan", "header",
               "numbers", "rows", "problems", "failure", "note", "suggest"),
  sheet_options = c("has_header", "skip_empty_rows", "range", "skip", "header_row", "tokens"),
  sheet = c("name", "hidden")
)

# Every key of a case no replay here reads, as a path.
unknown_keys <- function(case, workbook) {
  out <- character()
  check <- function(value, path, known) {
    if (is.list(value) && !is.null(names(value))) {
      unknown <- setdiff(names(value), known)
      if (length(unknown)) out <<- c(out, paste0(path, ".", unknown))
    }
  }
  name <- case$name %||% "a case with no name"
  check(case, name, if (workbook) known_keys$workbook else known_keys$delimited)
  if (workbook) {
    check(case$options, paste0(name, ".options"), known_keys$sheet_options)
    for (s in case$sheets) check(s, paste0(name, ".sheets[]"), known_keys$sheet)
  } else {
    check(case$dialect, paste0(name, ".dialect"), known_keys$dialect)
  }
  for (column in case$plan) {
    check(column, paste0(name, ".plan[]"), known_keys$plan)
    check(column$format, paste0(name, ".plan[].format"), known_keys$format)
    check(column$options, paste0(name, ".plan[].options"), known_keys$date)
    check(column$constraints, paste0(name, ".plan[].constraints"), known_keys$constraints)
  }
  for (row in case$rows) for (cell in row) check(cell, paste0(name, ".rows[][]"), known_keys$cell)
  for (p in case$problems) check(p, paste0(name, ".problems[]"), known_keys$problem)
  check(case$failure, paste0(name, ".failure"), known_keys$failure)
  if (!is.null(case$suggest)) {
    check(case$suggest, paste0(name, ".suggest"), known_keys$suggest)
    check(case$suggest$format, paste0(name, ".suggest.format"), known_keys$format)
    for (column in case$suggest$columns) {
      check(column, paste0(name, ".suggest.columns[]"), known_keys$suggested)
    }
  }
  out
}

# Every object in `value`, the first of each list only, as the path to it.
key_places <- function(value, path = list()) {
  if (is.list(value) && !is.null(names(value))) {
    c(list(path), unlist(lapply(names(value), function(key) {
      key_places(value[[key]], c(path, key))
    }), recursive = FALSE))
  } else if (is.list(value) && length(value)) {
    key_places(value[[1]], c(path, 1L))
  } else {
    list()
  }
}

# `value` with a key no replay reads added to the object at `place`.
with_junk <- function(value, place) {
  if (!length(place)) {
    value[["junk"]] <- TRUE
    return(value)
  }
  value[[place[[1]]]] <- with_junk(value[[place[[1]]]], place[-1])
  value
}

# Nothing in the corpus is passed over, and the check notices a key added anywhere: a failure
# message, or NULL.
keys_problem <- function(cases, workbook) {
  passed_over <- unlist(lapply(cases, unknown_keys, workbook = workbook))
  if (length(passed_over)) return(paste(utils::head(passed_over, 5), collapse = "; "))
  for (case in cases) {
    for (place in key_places(case)) {
      edited <- with_junk(case, place)
      unknown <- unknown_keys(edited, workbook)
      if (length(unknown) != 1L || !endsWith(unknown, ".junk")) {
        return(paste("a key at", paste(place, collapse = "/"), "is not noticed:",
                     paste(unknown, collapse = ", ")))
      }
    }
  }
  NULL
}

corpus_precisions <- c("seconds", "milliseconds", "microseconds", "nanoseconds")
corpus_orders <- c("year_month_day", "month_day_year", "day_month_year")
corpus_epochs <- c("y1900", "y1904")
corpus_policies <- c("0" = "western", "512" = "indian")

format_of <- function(spec) {
  if (is.null(spec)) {
    return(NULL)
  }
  policy <- corpus_policies[as.character(spec$flags - bitwAnd(spec$flags, 0xFF))]
  if (is.na(policy)) stop("the corpus declares a grouping policy this replay does not know")
  hypercast::num_format(spec$decimal_sep, spec$group_sep, bitwAnd(spec$flags, 0xFF),
                        currency = spec$currency %||% "", grouping = unname(policy))
}

date_options_of <- function(options) {
  if (is.null(options)) return(NULL)
  hypercast::date_options(as.character(unlist(options$flags)), options$pivot %||% 0L)
}

months_of <- function(options) {
  if (is.null(options$months)) return(NULL)
  hypercast::month_names(stats::setNames(
    vapply(options$months, function(m) as.integer(m[[1]]), 0L),
    vapply(options$months, function(m) m[[2]], "")
  ))
}

tokens_of <- function(entry) if (is.null(entry$tokens)) NULL else as.character(unlist(entry$tokens))

# A plan entry's constraints as hypercast's declaration for its door, or NULL.
constraints_of <- function(entry) {
  c <- entry$constraints
  if (is.null(c)) return(NULL)
  whole <- function(x) if (is.null(x)) NULL else as.numeric(x)
  hypercast::hypercast_constraints(
    entry$door, minimum = c$minimum, exclusive_minimum = c$exclusiveMinimum,
    maximum = c$maximum, exclusive_maximum = c$exclusiveMaximum, multiple_of = c$multipleOf,
    rounding = c$rounding %||% "strict", min_length = whole(c$minLength),
    max_length = whole(c$maxLength), trim = isTRUE(c$trim),
    case_sensitive = isTRUE(c$caseSensitive),
    enum = if (!is.null(c$enum)) as.character(unlist(c$enum))
  )
}

# A corpus plan entry as a column; `by_name` reads the source by its header name instead of
# its position where the header has that name once.
column_of <- function(entry, header = NULL, by_name = FALSE) {
  source <- entry$ordinal + 1L
  if (by_name && !is.null(header) && source <= length(header)) {
    name <- header[[source]]
    if (!is.na(name) && nzchar(name) && sum(header == name, na.rm = TRUE) == 1L) source <- name
  }
  na <- tokens_of(entry)
  options <- date_options_of(entry$options)
  months <- months_of(entry$options)
  constraints <- constraints_of(entry)
  switch(entry$door,
    unix = col_unix(source, corpus_precisions[[entry$precision]], na = na),
    excel_serial = col_excel_serial(source, corpus_epochs[[entry$epoch]], na = na),
    date = col_date(source, options = options, months = months, na = na),
    date_ordered = col_date(source, corpus_orders[[entry$order]], options = options,
                            months = months, na = na),
    datetime = col_datetime(source, corpus_orders[[entry$order]], options = options,
                            months = months, na = na),
    timestamp = col_timestamp(source, options = options, na = na),
    i8 = , i16 = , i32 = , i64 = , u8 = , u16 = , u32 = , u64 = , f32 = , f64 = ,
    decimal = get(paste0("col_", entry$door))(source, format_of(entry$format), na = na,
                                              constraints = constraints),
    text = col_text(source, na = na, constraints = constraints),
    get(paste0("col_", entry$door))(source, na = na)
  )
}

plan_of <- function(case, header = NULL, by_name = FALSE) {
  lapply(case$plan, column_of, header = header, by_name = by_name)
}

hex <- function(text) {
  as.raw(strtoi(substring(text, seq(1, nchar(text), 2), seq(2, nchar(text), 2)), 16L))
}

input_of <- function(case) {
  if (!is.null(case$input_hex)) hex(case$input_hex) else charToRaw(enc2utf8(case$input))
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

# The decimal's text from its parts: what the corpus's value says, built independently.
decimal_text <- function(cell) {
  digits <- cell$magnitude
  scale <- cell$scale
  if (scale > 0) {
    digits <- paste0(strrep("0", max(scale + 1L - nchar(digits), 0L)), digits)
    digits <- paste0(substr(digits, 1, nchar(digits) - scale), ".",
                     substr(digits, nchar(digits) - scale + 1L, nchar(digits)))
  }
  paste0(if (isTRUE(cell$negative)) "-", digits)
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
    i64 = , u64 = list(value = cell$value),
    decimal = list(value = decimal_text(cell)),
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

# The bytes a faulted cell's raw holds, as the corpus says them.
cell_bytes <- function(cell) {
  if (!is.null(cell$raw_hex)) hex(cell$raw_hex) else charToRaw(enc2utf8(cell$raw))
}

# A byte span over a cell's text, in the units its raw counts: characters for UTF-8, bytes
# for text that isn't.
span_of <- function(cell, fault) {
  bytes <- cell_bytes(cell)
  if (!validUTF8(rawToChar(bytes))) {
    return(as.integer(c(fault[[1]], fault[[2]])))
  }
  start <- fault[[1]]
  first <- chars_in(bytes, start)
  as.integer(c(first, chars_in(bytes, start + fault[[2]]) - first))
}

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
  checked <- inherits(v, "hypercast_checked_verdict")
  if (checked != !is.null(plain$keyword)) {
    return(sprintf("%s: a checked verdict without its keyword, or the other way about", label))
  }
  for (i in seq_along(cells)) {
    cell <- cells[[i]]
    origin <- cell$origin %||% "cell"
    if (!identical(as.character(plain$origin[[i]]), origin)) {
      out <- c(out, miss(i, "origin", plain$origin[[i]], origin))
    }
    if (checked) {
      out <- c(out, checked_mismatches(plain, i, door, cell, miss))
      if (cell$expect %in% c("ok", "violation")) next
    }
    want_ok <- identical(cell$expect, "ok")
    if (!identical(plain$ok[[i]], want_ok)) {
      out <- c(out, miss(i, "ok", plain$ok[[i]], paste(want_ok, cell$expect, plain$raw[[i]])))
      next
    }
    if (want_ok) {
      if (door == "decimal" && !identical(decimal_text(cell), cell$value)) {
        out <- c(out, miss(i, "the corpus's decimal", decimal_text(cell), cell$value))
      }
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
    detail <- cell$detail %||% NA_character_
    if (!identical(as.character(plain$detail[[i]]), detail)) {
      out <- c(out, miss(i, "detail", plain$detail[[i]], detail))
    }
    span <- c(0L, 0L)
    if (!is.null(cell$fault)) {
      raw <- plain$raw[[i]]
      if (is.na(raw) || !identical(charToRaw(raw), cell_bytes(cell))) {
        out <- c(out, miss(i, "raw", raw, cell$raw %||% cell$raw_hex))
      }
      span <- span_of(cell, cell$fault)
    } else if (cell$expect == "empty" && is.null(cell$origin) && !identical(plain$raw[[i]], "")) {
      out <- c(out, miss(i, "raw", plain$raw[[i]], ""))
    }
    got <- c(plain$offset[[i]], plain$length[[i]])
    if (!identical(got, span)) out <- c(out, miss(i, "span", got, span))
  }
  out
}

# A checked verdict's own columns at row i against the corpus's cell: a pass's `adjusted` and
# `enum_index` and value, a violation's keyword, value as cast, raw text and span, and nothing of
# either on a fault, which column_mismatches() then holds as any fault.
checked_mismatches <- function(plain, i, door, cell, miss) {
  out <- character()
  passed <- identical(cell$expect, "ok")
  violated <- identical(cell$expect, "violation")
  want <- list(ok = if (passed) TRUE else FALSE,
               adjusted = if (passed) isTRUE(cell$adjusted) else NA,
               enum_index = if (passed && !is.null(cell$enum_index)) {
                 as.integer(cell$enum_index) + 1L
               } else {
                 NA_integer_
               },
               keyword = if (violated) cell$keyword else NA_character_,
               reason = if (passed || violated) NA_character_ else cell$expect)
  got <- list(ok = plain$ok[[i]], adjusted = plain$adjusted[[i]],
              enum_index = plain$enum_index[[i]], keyword = plain$keyword[[i]],
              reason = as.character(plain$reason[[i]]))
  for (field in names(want)) {
    if (!identical(got[[field]], want[[field]])) {
      out <- c(out, miss(i, field, got[[field]], want[[field]]))
    }
  }
  if (!passed && !violated) {
    return(out)
  }
  if (!is.null(cell$bits)) {
    size <- if (door == "f32") 4L else 8L
    bits <- paste0("0x", paste(writeBin(plain$value[[i]], raw(), size = size, endian = "big"),
                               collapse = ""))
    if (!identical(bits, cell$bits)) out <- c(out, miss(i, "bits", bits, cell$bits))
  } else {
    for (field in names(fields <- expected_fields(door, cell))) {
      if (!identical(unclass(plain[[field]][[i]]), fields[[field]])) {
        out <- c(out, miss(i, field, plain[[field]][[i]], fields[[field]]))
      }
    }
  }
  if (violated) {
    raw <- plain$raw[[i]]
    if (is.na(raw) || !identical(charToRaw(raw), cell_bytes(cell))) {
      out <- c(out, miss(i, "raw", raw, cell$raw %||% cell$raw_hex))
    }
    span <- span_of(cell, cell$fault)
    if (!identical(c(plain$offset[[i]], plain$length[[i]]), span)) {
      out <- c(out, miss(i, "span", c(plain$offset[[i]], plain$length[[i]]), span))
    }
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

# problems() of the read's batches, one string per entry, in the corpus's terms re-expressed in
# this binding's: rows and columns from 1, spans in substr() units.
said_problems <- function(batches) {
  unlist(lapply(batches, function(b) {
    p <- problems(b)
    if (!nrow(p)) return(character())
    stopifnot(all(p$line == b$line[match(p$row, b$row)]))
    plan_column <- match(p$column, names(b$columns))
    ifelse(p$kind == "row",
           sprintf("row %s, line %s: width %s, found %s", p$row, p$line, p$expected, p$found),
           sprintf("row %s, line %s, column %s (source %s): %s %s %s, span %s+%s", p$row, p$line,
                   plan_column, p$source_column,
                   ifelse(is.na(p$keyword), as.character(p$reason),
                          paste0("violation:", p$keyword)),
                   p$detail, p$origin, p$offset, p$length))
  }))
}

# The corpus counts a sheet's source column from its A, wherever the range starts, as this
# binding does.
wanted_problems <- function(case) {
  vapply(case$problems, function(p) {
    if (is.null(p$plan_column)) {
      return(sprintf("row %s, line %s: width %s, found %s", p$row + 1, p$line, p$expected,
                     p$found))
    }
    span <- if (is.null(p$fault)) {
      c(0L, 0L)
    } else {
      span_of(case$rows[[p$row + 1]][[p$plan_column + 1]], p$fault)
    }
    sprintf("row %s, line %s, column %s (source %s): %s %s %s, span %s+%s", p$row + 1, p$line,
            p$plan_column + 1, p$source_column + 1,
            if (!is.null(p$keyword)) paste0("violation:", p$keyword) else p$reason,
            p$detail %||% NA,
            p$origin %||% "cell", span[[1]], span[[2]])
  }, "")
}

problem_mismatches <- function(batches, case, label) {
  got <- said_problems(batches) %||% character()
  want <- wanted_problems(case) %||% character()
  if (identical(got, want)) {
    character()
  } else {
    sprintf("%s: problems\n  %s\nwant\n  %s", label, paste(got, collapse = "\n  "),
            paste(want, collapse = "\n  "))
  }
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
  got <- list(failure$kind, failure$record, failure$line, failure$byte, failure$expected,
              failure$found)
  wanted <- list(want$kind, as.numeric(want$record), as.numeric(want$line),
                 as.numeric(want$byte), as.numeric(want$expected %||% 0),
                 as.numeric(want$found %||% 0))
  if (identical(got, wanted)) character() else sprintf("%s: failure %s, want %s", label,
                                                       paste(got, collapse = " "),
                                                       paste(wanted, collapse = " "))
}

# The delimited corpus.

dialect_of <- function(case) {
  d <- case$dialect
  delimited_dialect(d$separator, quoting = d$quoting, has_header = d$has_header,
                    skip_blank_lines = d$skip_blank_lines, skip = d$skip %||% 0,
                    header_row = d$header_row %||% 0,
                    fill = identical(d$short_rows, "pad"), long_rows = d$long_rows %||% "reject",
                    na = as.character(unlist(d$tokens)), encoding = d$encoding %||% "utf-8")
}

# The batches of a read as one table, as read_delimited() would bind them.
as_table <- function(batches, plan_size) {
  t <- list(line = unlist(lapply(batches, `[[`, "line")),
            columns = lapply(seq_len(plan_size), function(j) {
              do.call(rbind, lapply(batches, function(b) b$columns[[j]]))
            }))
  if (is.null(t$line)) t$line <- numeric()
  t
}

# Reads `case` through `open` (a function of the dialect returning a reader with no plan
# bound), checks the header, binds the plan and reads every batch; returns the disagreements.
replay <- function(case, label, open, batch_rows, by_name = FALSE) {
  reader <- open(dialect_of(case))
  on.exit(close(reader), add = TRUE)
  out <- character()
  want_header <- if (is.null(case$header)) NULL else as.character(unlist(case$header))
  if (!identical(header(reader), want_header)) {
    out <- sprintf("%s: header %s, want %s", label, paste(header(reader), collapse = "|"),
                   paste(want_header, collapse = "|"))
  }
  bind(reader, plan_of(case, header(reader), by_name))
  batches <- list()
  while (!is.null(batch <- read_batch(reader))) {
    if (length(batch$line) < 1L || length(batch$line) > batch_rows) {
      out <- c(out, sprintf("%s: a batch of %d rows", label, length(batch$line)))
    }
    batches[[length(batches) + 1L]] <- batch
  }
  # A structural failure is final: every later read is NULL, and the failure stays.
  first <- failure(reader)
  if (!is.null(read_batch(reader)) || !identical(failure(reader), first)) {
    out <- c(out, sprintf("%s: a read after the end changed something", label))
  }
  t <- as_table(batches, length(case$plan))
  if (length(case$rows)) {
    out <- c(out, table_mismatches(t, case, label))
  } else if (length(t$line)) {
    out <- c(out, sprintf("%s: %d rows, want none", label, length(t$line)))
  }
  c(out, problem_mismatches(batches, case, label), failure_mismatches(failure(reader), case, label))
}

delimited_cases <- read_corpus("delimited.json")

test_that("the replay reads every key the corpus has, and notices one added anywhere", {
  expect_null(keys_problem(delimited_cases, workbook = FALSE))
  # The negative control: a key it doesn't know is named.
  edited <- delimited_cases[[1]]
  edited$plan[[1]]$options$junk <- 1
  expect_identical(unknown_keys(edited, FALSE), paste0(edited$name, ".plan[].options.junk"))
})

test_that("the corpus has its cases, and reaches every door", {
  expect_identical(length(delimited_cases), 137L)
  expect_identical(sum(vapply(delimited_cases, function(case) !is.null(case$suggest), FALSE)),
                   19L)
  doors <- unique(unlist(lapply(delimited_cases, function(case) {
    vapply(case$plan, `[[`, "", "door")
  })))
  expect_setequal(doors, names(hypertabular:::door_codes))
})

for (case in delimited_cases) {
  test_that(case$name, {
    input <- input_of(case)
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
    # The one-call read agrees with the batch-at-a-time one, problems() and all.
    t <- read_delimited(input, plan_of(case), dialect_of(case))
    if (length(case$rows)) out <- c(out, table_mismatches(t, case, "read_delimited"))
    out <- c(out, problem_mismatches(list(t), case, "read_delimited"),
             failure_mismatches(t$failure, case, "read_delimited"))
    expect_identical(out, character())
  })
}

# What suggest_plan() reports, in the corpus's terms. `param` is the door's parameter, as every
# binding gives it; an ambiguous column's candidates are folded back into the core's mask, which
# the corpus records, as Go's corpus test does.
suggestion_of <- function(s) {
  if (is.null(s$report)) return(paste("failed:", format(s$failure)))
  r <- s$report
  mask <- vapply(seq_len(nrow(r)), function(k) {
    if (r$status[[k]] != "ambiguous") return("-")
    bits <- vapply(r$candidates[[k]], function(c) {
      if (is.character(c)) 2^match(c, hypertabular:::orders)
      else if (identical(c$packed, hypercast::num_format()$packed)) 2 else 4
    }, 0)
    as.character(sum(bits))
  }, "")
  sprintf("%s %s %s %s of %s, candidates %s", r$door, r$param, r$status, r$matched, r$sampled,
          mask)
}

# The corpus's `param` is the core's raw number: an ambiguous date's mask of its orders, a
# number's notation. The door's parameter is the mask's lowest order, and 0 for a number.
wanted_suggestion <- function(columns) {
  vapply(columns, function(c) {
    dated <- c$door %in% c("date_ordered", "datetime")
    numeric <- c$door %in% hypertabular:::numeric_doors
    param <- c$param
    if (dated && c$status == "ambiguous" && param != 0) param <- log2(bitwAnd(param, -param))
    if (numeric) param <- 0
    sprintf("%s %s %s %s of %s, candidates %s", c$door, param, c$status, c$matched, c$sampled,
            if (c$status == "ambiguous") c$param else "-")
  }, "")
}

# Each resolved number is planned in the format the corpus's param says, or the one declared;
# every other column has none.
notation_mismatches <- function(s, columns, declared, label) {
  out <- character()
  for (k in seq_along(columns)) {
    report <- s$report[k, ]
    planned <- report$format[[1]]
    if (report$status == "resolved" && report$door %in% hypertabular:::numeric_doors) {
      wanted <- declared %||% if (columns[[k]]$param == 4) hypercast::num_format(",", ".") else
        hypercast::num_format()
      packed <- s$plan[[k]]$format
      if (!identical(planned$packed, wanted$packed) || !identical(packed, wanted$packed)) {
        out <- c(out, sprintf("%s: column %d planned in another notation", label, k))
      }
    } else if (!is.null(planned)) {
      out <- c(out, sprintf("%s: column %d has a notation", label, k))
    }
  }
  out
}

test_that("the corpus says what to suggest", {
  expect_gt(sum(vapply(delimited_cases, function(case) !is.null(case$suggest), FALSE)), 0L)
})

for (case in Filter(function(case) !is.null(case$suggest), delimited_cases)) {
  test_that(paste("suggest:", case$name), {
    expected <- case$suggest
    input <- input_of(case)
    dialect <- dialect_of(case)
    declared <- format_of(expected$format)
    sample <- expected$sample_rows %||% 1000
    s <- suggest_plan(input, dialect, sample_rows = sample, format = declared)
    expect_identical(suggestion_of(s), wanted_suggestion(expected$columns))
    expect_identical(s$truncated, expected$truncated)
    expect_identical(notation_mismatches(s, expected$columns, declared, case$name), character())
    path <- tempfile(fileext = ".csv")
    on.exit(unlink(path), add = TRUE)
    writeBin(input, path)
    expect_identical(suggest_plan(path, dialect, sample_rows = sample, format = declared), s)
    # The printed plan is code that makes the plan.
    printed <- utils::capture.output(print(s))
    expect_identical(eval(parse(text = printed)), s$plan)
    # The plan reads back refusing nothing it resolved, among the rows it sampled.
    t <- read_delimited(input, s$plan, dialect)
    p <- problems(t)
    resolved <- s$report$name[s$report$status == "resolved" & s$report$door != "text"]
    typed <- which(s$report$status == "resolved" & s$report$door != "text")
    planned <- match(p$column, names(t$columns))
    refused <- p[p$kind == "cell" & p$row <= s$rows & planned %in% typed, ]
    expect_identical(nrow(refused), 0L, label = paste(case$name, paste(resolved, collapse = ",")))
  })
}

test_that("a suggestion fails only where a read of the case fails", {
  planned <- 0L
  failed <- 0L
  for (case in delimited_cases) {
    s <- suggest_plan(input_of(case), dialect_of(case))
    if (is.null(s$failure)) {
      planned <- planned + 1L
    } else {
      failed <- failed + 1L
      expect_false(is.null(case$failure), label = case$name)
      expect_null(s$plan)
    }
  }
  expect_true(planned > 80L && failed > 0L)
})

# The workbook corpus.

workbook_cases <- read_corpus("workbook.json")
books <- new.env()

book_of <- function(file) {
  if (is.null(books[[file]])) books[[file]] <- workbook(file.path(corpus_dir(), file))
  books[[file]]
}

# The sheet options a case declares, as sheet_reader()'s arguments.
sheet_args <- function(options) {
  list(has_header = options$has_header, skip_empty_rows = options$skip_empty_rows,
       skip = options$skip %||% 0, header_row = options$header_row %||% 0,
       na = as.character(unlist(options$tokens)), range = options$range)
}

# The disagreements of one read of `case`.
replay_sheet <- function(case, label, stingy) {
  old <- options(hypertabular.stingy = stingy)
  on.exit(options(old), add = TRUE)
  path <- file.path(corpus_dir(), case$file)
  book <- if (stingy) workbook(path) else book_of(case$file)
  if (stingy) on.exit(close(book), add = TRUE)
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
  args <- sheet_args(case$options)
  want_header <- if (case$options$has_header) as.character(unlist(case$header))
  if (!stingy) {
    t <- do.call(read_sheet, c(list(book, index, plan_of(case)), args))
    header <- t$header
    batches <- list(t)
  } else {
    name <- want_sheets$name[[index]]
    which <- if (sum(want_sheets$name == name) == 1L) name else index
    reader <- do.call(sheet_reader, c(list(book, which, batch_rows = 7L), args))
    on.exit(close(reader), add = TRUE)
    header <- header(reader)
    bind(reader, plan_of(case, header, by_name = TRUE))
    batches <- list()
    while (!is.null(batch <- read_batch(reader))) {
      if (length(batch$line) > 7L) out <- c(out, sprintf("%s: a batch too long", label))
      batches[[length(batches) + 1L]] <- batch
    }
    t <- as_table(batches, length(case$plan))
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
  sheets <- unique(vapply(batches, function(b) b$context$sheet %||% NA_character_, ""))
  if (length(batches) && !identical(sheets, want_sheets$name[[index]])) {
    out <- c(out, sprintf("%s: problems name sheet %s", label, paste(sheets, collapse = ",")))
  }
  c(out, problem_mismatches(batches, case, label),
    failure_mismatches(t$failure, case, label))
}

test_that("the replay reads every key the workbook corpus has, and notices one added anywhere", {
  expect_null(keys_problem(workbook_cases, workbook = TRUE))
})

test_that("the corpus has its cases, over every package, through every door", {
  expect_identical(length(workbook_cases), 404L)
  files <- unique(vapply(workbook_cases, `[[`, "", "file"))
  expect_identical(length(files), 42L)
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
    if (!is.null(case$suggest)) {
      expected <- case$suggest
      declared <- format_of(expected$format)
      s <- do.call(suggest_plan, c(list(book_of(case$file), sheet = case$sheet + 1L,
                                        format = declared), sheet_args(case$options)))
      if (!identical(suggestion_of(s), wanted_suggestion(expected$columns)) ||
            !identical(s$truncated, expected$truncated)) {
        out <- c(out, sprintf("%s: suggest says %s (truncated %s)", case$name,
                              paste(suggestion_of(s), collapse = "; "), s$truncated))
      }
      out <- c(out, notation_mismatches(s, expected$columns, declared, case$name))
    }
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
  on.exit(close(book), add = TRUE)
  for (i in seq_len(nrow(book$sheets))) {
    reader <- sheet_reader(book, i, has_header = FALSE, batch_rows = 3L)
    bind(reader, list(col_text(1), col_f64(2), col_date(3), col_text(30)))
    while (!is.null(read_batch(reader))) NULL
    close(reader)
  }
}

test_that("no truncation of a delimited corpus input signals anything", {
  seen <- character()
  for (case in delimited_cases) {
    input <- input_of(case)
    dialect <- dialect_of(case)
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
        close(reader)
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
