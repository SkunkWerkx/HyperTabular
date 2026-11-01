#' @details
#' Readers for delimited text ([delimited_reader()], [read_delimited()]) and for XLSX, ODS
#' and Numbers workbooks ([workbook()], [sheet_reader()], [read_sheet()]), through a plan of
#' columns that each name a source column and the HyperCast door to cast it with
#' ([col_bool()] and its siblings). Every cell comes back as a HyperCast verdict, in the
#' `hypercast_verdict` data frames the hypercast package's own doors return: the value, or a
#' fault with its reason and the offending span of the cell's text. A file that is
#' structurally broken comes back as a [hypertabular_failure] value, after every intact row
#' before it. No read signals an R error for anything in its data; errors are reserved for
#' caller bugs in what declares how to read it (a dialect, a plan, a sheet), as in every
#' other HyperTabular binding.
#' @keywords internal
"_PACKAGE"

#' @useDynLib hypertabular, .registration = TRUE, .fixes = "C_"
#' @importFrom hypercast is_ok
NULL
