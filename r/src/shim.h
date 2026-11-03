/*
 * The shim's .Call entry points, defined in hypertabular.c and registered in init.c. The
 * core's own C ABI is include/hypertabular.h, cbindgen's, staged by tools/vendor.sh.
 */
#ifndef HYPERTABULAR_R_SHIM_H
#define HYPERTABULAR_R_SHIM_H

#include <Rinternals.h>

SEXP C_delimited_new(SEXP dialect, SEXP stingy);
SEXP C_delimited_feed(SEXP ptr, SEXP chunk);
SEXP C_delimited_header(SEXP ptr);
SEXP C_delimited_bind(SEXP ptr, SEXP specs, SEXP batch_rows, SEXP per_row);
SEXP C_delimited_fill(SEXP ptr);
SEXP C_delimited_width(SEXP ptr);
SEXP C_workbook_open(SEXP container, SEXP stingy);
SEXP C_sheet_new(SEXP book, SEXP part, SEXP index, SEXP has_header, SEXP skip_empty_rows,
                 SEXP stingy);
SEXP C_sheet_header(SEXP ptr);
SEXP C_sheet_bind(SEXP ptr, SEXP specs, SEXP batch_rows, SEXP width);
SEXP C_sheet_fill(SEXP ptr);
SEXP C_native_version(void);

#endif
