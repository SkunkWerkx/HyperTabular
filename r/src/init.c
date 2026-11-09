#include <R.h>
#include <R_ext/Rdynload.h>
#include <Rinternals.h>
#include <stddef.h>

#include "shim.h"

static const R_CallMethodDef CALL_ENTRIES[] = {
    {"layout", (DL_FUNC)&C_layout, 0},
    {"batch_check", (DL_FUNC)&C_batch_check, 5},
    {"judge", (DL_FUNC)&C_judge, 2},
    {"range_parse", (DL_FUNC)&C_range_parse, 1},
    {"faults", (DL_FUNC)&C_faults, 5},
    {"suggest", (DL_FUNC)&C_suggest, 4},
    {"delimited_new", (DL_FUNC)&C_delimited_new, 4},
    {"delimited_feed", (DL_FUNC)&C_delimited_feed, 2},
    {"delimited_pull", (DL_FUNC)&C_delimited_pull, 5},
    {"delimited_header", (DL_FUNC)&C_delimited_header, 1},
    {"delimited_bind", (DL_FUNC)&C_delimited_bind, 6},
    {"delimited_fill", (DL_FUNC)&C_delimited_fill, 2},
    {"delimited_ack", (DL_FUNC)&C_delimited_ack, 1},
    {"delimited_unack", (DL_FUNC)&C_delimited_unack, 1},
    {"delimited_width", (DL_FUNC)&C_delimited_width, 1},
    {"delimited_close", (DL_FUNC)&C_delimited_close, 1},
    {"workbook_open", (DL_FUNC)&C_workbook_open, 2},
    {"workbook_close", (DL_FUNC)&C_workbook_close, 1},
    {"sheet_new", (DL_FUNC)&C_sheet_new, 6},
    {"sheet_header", (DL_FUNC)&C_sheet_header, 1},
    {"sheet_bind", (DL_FUNC)&C_sheet_bind, 7},
    {"sheet_fill", (DL_FUNC)&C_sheet_fill, 2},
    {"sheet_ack", (DL_FUNC)&C_sheet_ack, 1},
    {"sheet_unack", (DL_FUNC)&C_sheet_unack, 1},
    {"sheet_close", (DL_FUNC)&C_sheet_close, 1},
    {"one_dot_zero_delimited", (DL_FUNC)&C_one_dot_zero_delimited, 3},
    {"one_dot_zero_sheet", (DL_FUNC)&C_one_dot_zero_sheet, 4},
    {"interrupt_point", (DL_FUNC)&C_interrupt_point, 0},
    {"regular_file", (DL_FUNC)&C_regular_file, 1},
    {"native_version", (DL_FUNC)&C_native_version, 0},
    {NULL, NULL, 0},
};

void R_init_hypertabular(DllInfo *dll) {
  R_registerRoutines(dll, NULL, CALL_ENTRIES, NULL, NULL);
  R_useDynamicSymbols(dll, FALSE);
  R_forceSymbols(dll, TRUE);
}
