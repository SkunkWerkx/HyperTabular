package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import java.util.Objects;

/**
 * One output column of a plan: which source column it reads, the door it casts through,
 * and — for the numeric doors — the notation. A plan is a projection: a forty-column file
 * can be read into five typed columns, in any order, and a source column can be read
 * through more than one door.
 *
 * <p>Built by the factory for its door, one per door, named as HyperCast's {@code Cast}
 * names them. The numeric doors take a HyperCast {@link NumFormat}, or none for
 * {@link NumFormat#INVARIANT}; the doors that declare something — a Unix precision, a date
 * order, an Excel date system — take HyperCast's own enum for it. Immutable, and equal to
 * another column that reads the same source column the same way.
 */
public final class Column {
    private final int ordinal;
    private final Door door;
    private final int declared;
    private final NumFormat format;

    private Column(int ordinal, Door door, int declared, NumFormat format) {
        if (ordinal < 0) {
            throw new IllegalArgumentException("ordinal must not be negative; got " + ordinal);
        }
        Objects.requireNonNull(format, "format");
        // The core reads a zero decimal separator as "no format declared" — the invariant
        // one — so a format that really declared U+0000 would be silently replaced.
        if (format.decimalSeparator() == 0) {
            throw new IllegalArgumentException("The decimal separator must not be U+0000");
        }
        this.ordinal = ordinal;
        this.door = door;
        this.declared = declared;
        this.format = format;
    }

    private static Column plain(int ordinal, Door door) {
        return new Column(ordinal, door, 0, NumFormat.INVARIANT);
    }

    /**
     * Zero-based ordinal of the source column. Past a record's last cell reads as empty.
     *
     * @return the source column's ordinal
     */
    public int ordinal() {
        return ordinal;
    }

    /**
     * The door this column is cast through.
     *
     * @return the door
     */
    public Door door() {
        return door;
    }

    /**
     * The numeric notation, read by the numeric doors; {@link NumFormat#INVARIANT} on every
     * other.
     *
     * @return the declared notation
     */
    public NumFormat format() {
        return format;
    }

    /** What the door declares beside itself, as the core numbers it; {@code 0} for none. */
    int declared() {
        return declared;
    }

    /**
     * A {@link Boolean} column.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column bool(int ordinal) {
        return plain(ordinal, Door.BOOL);
    }

    /**
     * A signed 8-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column i8(int ordinal) {
        return plain(ordinal, Door.I8);
    }

    /**
     * A signed 8-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column i8(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.I8, 0, format);
    }

    /**
     * A signed 16-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column i16(int ordinal) {
        return plain(ordinal, Door.I16);
    }

    /**
     * A signed 16-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column i16(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.I16, 0, format);
    }

    /**
     * A signed 32-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column i32(int ordinal) {
        return plain(ordinal, Door.I32);
    }

    /**
     * A signed 32-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column i32(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.I32, 0, format);
    }

    /**
     * A signed 64-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column i64(int ordinal) {
        return plain(ordinal, Door.I64);
    }

    /**
     * A signed 64-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column i64(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.I64, 0, format);
    }

    /**
     * An unsigned 8-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column u8(int ordinal) {
        return plain(ordinal, Door.U8);
    }

    /**
     * An unsigned 8-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column u8(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.U8, 0, format);
    }

    /**
     * An unsigned 16-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column u16(int ordinal) {
        return plain(ordinal, Door.U16);
    }

    /**
     * An unsigned 16-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column u16(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.U16, 0, format);
    }

    /**
     * An unsigned 32-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column u32(int ordinal) {
        return plain(ordinal, Door.U32);
    }

    /**
     * An unsigned 32-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column u32(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.U32, 0, format);
    }

    /**
     * An unsigned 64-bit column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column u64(int ordinal) {
        return plain(ordinal, Door.U64);
    }

    /**
     * An unsigned 64-bit column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column u64(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.U64, 0, format);
    }

    /**
     * A {@code float} column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column f32(int ordinal) {
        return plain(ordinal, Door.F32);
    }

    /**
     * A {@code float} column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column f32(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.F32, 0, format);
    }

    /**
     * A {@code double} column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column f64(int ordinal) {
        return plain(ordinal, Door.F64);
    }

    /**
     * A {@code double} column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column f64(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.F64, 0, format);
    }

    /**
     * An exact decimal column under the invariant notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column decimal(int ordinal) {
        return plain(ordinal, Door.DECIMAL);
    }

    /**
     * An exact decimal column under a declared notation.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @return the column
     */
    public static Column decimal(int ordinal, NumFormat format) {
        return new Column(ordinal, Door.DECIMAL, 0, format);
    }

    /**
     * A {@link java.util.UUID} column.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column uuid(int ordinal) {
        return plain(ordinal, Door.UUID);
    }

    /**
     * An RFC 3339 instant column.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column timestamp(int ordinal) {
        return plain(ordinal, Door.TIMESTAMP);
    }

    /**
     * A Unix-epoch column at the declared precision — never guessed from magnitude.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param precision the declared unit of the epoch value
     * @return the column
     */
    public static Column unix(int ordinal, UnixPrecision precision) {
        // HyperCast's enums carry the core's discriminants, but not publicly. The switches
        // here and below have no default on purpose: a constant HyperCast adds fails this
        // file's compile rather than being numbered by position.
        int declared = switch (Objects.requireNonNull(precision, "precision")) {
            case SECONDS -> 1;
            case MILLISECONDS -> 2;
            case MICROSECONDS -> 3;
            case NANOSECONDS -> 4;
        };
        return new Column(ordinal, Door.UNIX, declared, NumFormat.INVARIANT);
    }

    /**
     * An Excel date-serial column under the declared date system.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param epoch the declared date system
     * @return the column
     */
    public static Column excelSerial(int ordinal, ExcelEpoch epoch) {
        int declared = switch (Objects.requireNonNull(epoch, "epoch")) {
            case Y1900 -> 1;
            case Y1904 -> 2;
        };
        return new Column(ordinal, Door.EXCEL_SERIAL, declared, NumFormat.INVARIANT);
    }

    /**
     * A strict {@code yyyy-MM-dd} date column.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column date(int ordinal) {
        return plain(ordinal, Door.DATE);
    }

    /**
     * A separated-date column under the declared field order.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param order the declared field order
     * @return the column
     */
    public static Column date(int ordinal, DateOrder order) {
        return new Column(ordinal, Door.DATE_ORDERED, code(order), NumFormat.INVARIANT);
    }

    /**
     * A zone-less civil date-time column under the declared field order.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param order the declared field order of the date part
     * @return the column
     */
    public static Column dateTime(int ordinal, DateOrder order) {
        return new Column(ordinal, Door.DATETIME, code(order), NumFormat.INVARIANT);
    }

    /**
     * A 24-hour time-of-day column.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column time(int ordinal) {
        return plain(ordinal, Door.TIME);
    }

    /**
     * A duration column.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column duration(int ordinal) {
        return plain(ordinal, Door.DURATION);
    }

    /**
     * A text column: the cell's bytes themselves, untrimmed.
     *
     * @param ordinal zero-based ordinal of the source column
     * @return the column
     */
    public static Column text(int ordinal) {
        return plain(ordinal, Door.TEXT);
    }

    private static int code(DateOrder order) {
        return switch (Objects.requireNonNull(order, "order")) {
            case YEAR_MONTH_DAY -> 1;
            case MONTH_DAY_YEAR -> 2;
            case DAY_MONTH_YEAR -> 3;
        };
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Column that
                && ordinal == that.ordinal
                && door == that.door
                && declared == that.declared
                && format.equals(that.format);
    }

    @Override
    public int hashCode() {
        return Objects.hash(ordinal, door, declared, format);
    }

    @Override
    public String toString() {
        return "Column[ordinal=" + ordinal + ", door=" + door
                + (declared == 0 ? "" : ", declared=" + declared)
                + (format.equals(NumFormat.INVARIANT) ? "" : ", format=" + format) + "]";
    }
}
