/*
 * The shim's .Call entry points, defined in hypertabular.c and registered in init.c. The
 * core's own C ABI is include/hypertabular.h, cbindgen's, staged by tools/vendor.sh.
 */
#ifndef HYPERTABULAR_R_SHIM_H
#define HYPERTABULAR_R_SHIM_H

#include <Rinternals.h>

SEXP C_layout(void);
SEXP C_batch_check(SEXP specs, SEXP lists, SEXP batch_rows, SEXP buffer_bytes, SEXP flags);
SEXP C_judge(SEXP specs, SEXP lists);
SEXP C_range_parse(SEXP text);
SEXP C_faults(SEXP columns, SEXP rows, SEXP widths, SEXP expected, SEXP long_rows);
SEXP C_suggest(SEXP cells, SEXP typed, SEXP rows, SEXP formats);
SEXP C_delimited_new(SEXP options, SEXP tokens, SEXP encoding, SEXP stingy);
SEXP C_delimited_feed(SEXP ptr, SEXP chunk);
SEXP C_delimited_pull(SEXP ptr, SEXP con, SEXP chunk, SEXP whole, SEXP env);
SEXP C_delimited_header(SEXP ptr);
SEXP C_delimited_bind(SEXP ptr, SEXP specs, SEXP lists, SEXP raws, SEXP batch_rows, SEXP per_row);
SEXP C_delimited_fill(SEXP ptr, SEXP sample);
SEXP C_delimited_ack(SEXP ptr);
SEXP C_delimited_unack(SEXP ptr);
SEXP C_delimited_width(SEXP ptr);
SEXP C_delimited_close(SEXP ptr);
SEXP C_workbook_open(SEXP container, SEXP stingy);
SEXP C_workbook_close(SEXP ptr);
SEXP C_sheet_new(SEXP book, SEXP part, SEXP index, SEXP options, SEXP tokens, SEXP stingy);
SEXP C_sheet_header(SEXP ptr);
SEXP C_sheet_bind(SEXP ptr, SEXP specs, SEXP lists, SEXP raws, SEXP batch_rows, SEXP width,
                  SEXP widths);
SEXP C_sheet_fill(SEXP ptr, SEXP sample);
SEXP C_sheet_ack(SEXP ptr);
SEXP C_sheet_unack(SEXP ptr);
SEXP C_sheet_close(SEXP ptr);
SEXP C_one_dot_zero_delimited(SEXP input, SEXP dialect, SEXP specs);
SEXP C_one_dot_zero_sheet(SEXP book, SEXP part, SEXP index, SEXP specs);
SEXP C_interrupt_point(void);
SEXP C_regular_file(SEXP path);
SEXP C_native_version(void);

#endif
