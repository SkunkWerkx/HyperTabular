package io.github.skunkwerkx.hypertabular;

/** Which kind of workbook a container holds — told by what is in it, never by its name. */
public enum WorkbookFormat {
    /** Office Open XML ({@code .xlsx}). */
    XLSX,
    /** OpenDocument ({@code .ods}). */
    ODS
}
