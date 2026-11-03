#include <R.h>
#include <R_ext/Rdynload.h>
#include <Rinternals.h>
#include <stddef.h>

#include "shim.h"

static const R_CallMethodDef CALL_ENTRIES[] = {
    {"delimited_new", (DL_FUNC)&C_delimited_new, 2},
    {"delimited_feed", (DL_FUNC)&C_delimited_feed, 2},
    {"delimited_header", (DL_FUNC)&C_delimited_header, 1},
    {"delimited_bind", (DL_FUNC)&C_delimited_bind, 4},
    {"delimited_fill", (DL_FUNC)&C_delimited_fill, 1},
    {"delimited_width", (DL_FUNC)&C_delimited_width, 1},
    {"workbook_open", (DL_FUNC)&C_workbook_open, 2},
    {"sheet_new", (DL_FUNC)&C_sheet_new, 6},
    {"sheet_header", (DL_FUNC)&C_sheet_header, 1},
    {"sheet_bind", (DL_FUNC)&C_sheet_bind, 4},
    {"sheet_fill", (DL_FUNC)&C_sheet_fill, 1},
    {"native_version", (DL_FUNC)&C_native_version, 0},
    {NULL, NULL, 0},
};

void R_init_hypertabular(DllInfo *dll) {
  R_registerRoutines(dll, NULL, CALL_ENTRIES, NULL, NULL);
  R_useDynamicSymbols(dll, FALSE);
  R_forceSymbols(dll, TRUE);
}
