package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.FaultDetail;
import java.util.OptionalInt;

/**
 * One entry of {@link Batch#faults()}: a cell that went wrong, or a row longer than the width.
 *
 * @param row the row, from zero, in the batch
 * @param line delimited text: the record's first physical line; a sheet: its absolute row; from
 *     one
 * @param planColumn the column of the plan; empty for an entry about the row as a whole
 * @param sourceColumn the source column, from zero; a sheet's own, so {@code A} is 0 wherever a
 *     range starts; empty for an entry about the row as a whole
 * @param reason why the verdict is what it is: malformed, out of range, or empty for a formula
 *     with no cached value; {@code null} for an entry about the row as a whole
 * @param detail what failed, or {@code null} where nothing says; one this binding does not name
 *     keeps its code
 * @param origin where the verdict came from; an entry about the row as a whole says
 *     {@link Origin#CELL}
 * @param offset byte offset of the offending span in the cell's decoded, unescaped text; 0 where
 *     there is none
 * @param length byte length of the offending span; 0 where there is none
 * @param expected a long row's width by its header or first record; otherwise 0
 * @param found how many cells a long row had; otherwise 0
 */
public record Problem(
        int row,
        int line,
        OptionalInt planColumn,
        OptionalInt sourceColumn,
        CastFailure reason,
        FaultDetail detail,
        Origin origin,
        int offset,
        int length,
        int expected,
        int found) {}
