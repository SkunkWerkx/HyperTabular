# Writes inst/AUTHORS: one line per vendored crate (the dependency closure of the rcore
# wrapper, without rcore itself, the hypertabular core, which is this project's own, and the
# hypercast crate the core judges every cell with, its upstream from the same authors), from
# cargo metadata. Run from the package root by tools/vendor.sh, which leaves src/rust/vendor
# in place for it: the metadata is read offline against that directory, since the lock names
# the crates.io source for the core at a version that is not published while a release packs
# this package. Neither crate has other dependencies today, so the file says so in one line; a
# crate either comes to depend on gets its line here, and its authors a place in DESCRIPTION.
# The metadata goes through a file: on Windows `system2(stdout = TRUE)` splits captured lines
# at 8095 bytes, and the break can land inside a string.
metadata <- tempfile(fileext = ".json")
vendored <- c("--offline",
              "--config", shQuote('source.crates-io.replace-with="vendored-sources"'),
              "--config", shQuote('source.vendored-sources.directory="src/rust/vendor"'))
status <- system2("cargo", c("metadata", "--format-version", "1", "--locked", vendored,
                             "--manifest-path", "src/rust/Cargo.toml"), stdout = metadata)
if (status != 0L) stop("vendor-authors.R: cargo metadata exited with status ", status)
meta <- jsonlite::fromJSON(metadata, simplifyVector = FALSE)
own <- c("rcore", "hypertabular", "hypercast")
pkgs <- Filter(function(p) !p$name %in% own, meta$packages)
pkgs <- pkgs[order(vapply(pkgs, function(p) p$name, ""))]
field <- function(x, missing) {
  if (is.null(x) || !length(x)) missing else paste(unlist(x), collapse = ", ")
}
credit <- function(p) {
  if (is.null(p$authors) || !length(p$authors)) {
    stop("vendor-authors.R: ", p$name, " declares no authors; credit it as its license names them")
  }
  paste(unlist(p$authors), collapse = ", ")
}
lines <- vapply(pkgs, function(p) {
  sprintf("%s %s: %s (%s, %s)", p$name, p$version, credit(p),
          field(p$license, "no license field"), field(p$repository, "no repository field"))
}, "")
if (!length(lines)) {
  lines <- paste("No third-party Rust crates are vendored: src/rust/vendor.tar.xz carries only",
                 "the hypertabular core and the hypercast crate it judges every cell with, both",
                 "the work of this package's authors (Skunk Werkx, MIT),",
                 "but for the Unicode data below.")
}
dir.create("inst", showWarnings = FALSE)
# The core also carries Unicode's case folding data, so it is credited on every run.
unicode <- paste("The vendored hypercast crate's case folding (unicode/CaseFolding-17.0.0.txt and",
                 "src/fold_table.rs, generated from it) is from the Unicode Character Database,",
                 "(c) Unicode, Inc., Unicode License v3: https://www.unicode.org/license.txt")
writeLines(c(lines, unicode), "inst/AUTHORS", useBytes = TRUE)
