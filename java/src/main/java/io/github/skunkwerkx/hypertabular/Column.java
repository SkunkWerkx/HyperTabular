package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.Constraints;
import io.github.skunkwerkx.hypercast.DateOptions;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.MonthNames;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import java.math.BigDecimal;
import java.util.Locale;
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
 *
 * <p>A numeric or text column may declare HyperCast's {@link Constraints}, the OpenAPI keywords,
 * through the factory of its door that takes them: each cell is then cast and judged, and read with
 * {@link Batch#getChecked}. A declaration built for another door is refused: Java's type alone can't
 * tell {@code Constraints.u8()}'s from {@code Constraints.i32()}'s, so the factory reads
 * {@link Constraints#door()}.
 */
public final class Column {
    private final int ordinal;
    private final Door door;
    private final int declared;
    private final NumFormat format;
    /** The column's own tokens; {@code null} inherits the dialect's or the sheet's. */
    private final Tokens tokens;

    private final DateOptions dateOptions;
    private final MonthNames months;
    /** The column's constraints, built for its door; {@code null} for none. */
    private final Constraints<?> constraints;

    private Column(int ordinal, Door door, int declared, NumFormat format) {
        this(ordinal, door, declared, format, null, null, null, null);
    }

    private Column(
            int ordinal,
            Door door,
            int declared,
            NumFormat format,
            Tokens tokens,
            DateOptions dateOptions,
            MonthNames months,
            Constraints<?> constraints) {
        if (ordinal < 0) {
            throw new IllegalArgumentException("ordinal must not be negative; got " + ordinal);
        }
        // A NumFormat validated itself when it was built, as HyperCast's own doors validate
        // it, and its canonical constructor and Grouping.of refuse a grouping word no door
        // accepts, so a column cannot hold one: the core refuses nothing a NumFormat can hold.
        Objects.requireNonNull(format, "format");
        this.ordinal = ordinal;
        this.door = door;
        this.declared = declared;
        this.format = format;
        this.tokens = tokens;
        this.dateOptions = dateOptions;
        this.months = months;
        this.constraints = constraints;
    }

    /** {@code column}, judged against {@code constraints}, which must have been built for its door. */
    private static Column checked(Column column, Constraints<?> constraints) {
        Objects.requireNonNull(constraints, "constraints");
        String door = column.door.name().toLowerCase(Locale.ROOT);
        if (!constraints.door().equals(door)) {
            throw new IllegalArgumentException(
                    "Constraints built for the " + constraints.door() + " door were passed to a " + door + " column");
        }
        return new Column(
                column.ordinal,
                column.door,
                column.declared,
                column.format,
                column.tokens,
                column.dateOptions,
                column.months,
                constraints);
    }

    /**
     * Whether the column declares {@link Constraints}: its cells are then read with
     * {@link Batch#getChecked}, and {@link Batch#get} refuses it.
     *
     * @return {@code true} for a column that declares constraints
     */
    public boolean isChecked() {
        return constraints != null;
    }

    /** The column's constraints, or {@code null} for none. */
    Constraints<?> constraints() {
        return constraints;
    }

    /** The column a suggestion plans: a door and what it declares, as the core numbers them. */
    static Column planned(int ordinal, Door door, int declared, NumFormat format) {
        return new Column(ordinal, door, declared, format);
    }

    private static Column plain(int ordinal, Door door) {
        return new Column(ordinal, door, 0, NumFormat.INVARIANT);
    }

    /**
     * Zero-based ordinal of the source column. Past a record's last cell reads as empty; 2^20
     * and past, a plan is refused at bind.
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

    /**
     * This column with a list of its own of what stands for no value, ahead of the dialect's or
     * the sheet's. With no tokens, nothing does.
     *
     * @param tokens the tokens, matched as their exact UTF-8 bytes
     * @return the changed column
     */
    public Column withTokens(String... tokens) {
        return new Column(ordinal, door, declared, format, Tokens.of(tokens), dateOptions, months, constraints);
    }

    /**
     * This column's own list of what stands for no value.
     *
     * @return the tokens, or {@code null} if the column inherits the dialect's or the sheet's
     */
    public Tokens tokens() {
        return tokens;
    }

    /**
     * This column with the date forms it also accepts: a compact date, a space before the time, an
     * hour-only offset, a pivot for a two-digit year. Read by the date doors alone.
     *
     * @param options the options
     * @return the changed column
     */
    public Column withDateOptions(DateOptions options) {
        return new Column(
                ordinal,
                door,
                declared,
                format,
                tokens,
                Objects.requireNonNull(options, "options"),
                months,
                constraints);
    }

    /**
     * This column with the date forms it also accepts and the month names it reads.
     *
     * @param options the options
     * @param months the month names
     * @return the changed column
     */
    public Column withDateOptions(DateOptions options, MonthNames months) {
        return new Column(
                ordinal,
                door,
                declared,
                format,
                tokens,
                Objects.requireNonNull(options, "options"),
                Objects.requireNonNull(months, "months"),
                constraints);
    }

    /**
     * The date forms this column also accepts.
     *
     * @return the options, or {@code null} for none
     */
    public DateOptions dateOptions() {
        return dateOptions;
    }

    /**
     * The month names this column reads.
     *
     * @return the names, or {@code null} for none
     */
    public MonthNames months() {
        return months;
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
     * A signed 8-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.i8()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i8(int ordinal, Constraints<Byte> constraints) {
        return checked(i8(ordinal), constraints);
    }

    /**
     * A signed 8-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.i8()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i8(int ordinal, NumFormat format, Constraints<Byte> constraints) {
        return checked(i8(ordinal, format), constraints);
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
     * A signed 16-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.i16()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i16(int ordinal, Constraints<Short> constraints) {
        return checked(i16(ordinal), constraints);
    }

    /**
     * A signed 16-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.i16()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i16(int ordinal, NumFormat format, Constraints<Short> constraints) {
        return checked(i16(ordinal, format), constraints);
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
     * A signed 32-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.i32()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i32(int ordinal, Constraints<Integer> constraints) {
        return checked(i32(ordinal), constraints);
    }

    /**
     * A signed 32-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.i32()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i32(int ordinal, NumFormat format, Constraints<Integer> constraints) {
        return checked(i32(ordinal, format), constraints);
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
     * A signed 64-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.i64()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i64(int ordinal, Constraints<Long> constraints) {
        return checked(i64(ordinal), constraints);
    }

    /**
     * A signed 64-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.i64()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column i64(int ordinal, NumFormat format, Constraints<Long> constraints) {
        return checked(i64(ordinal, format), constraints);
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
     * An unsigned 8-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.u8()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u8(int ordinal, Constraints<Integer> constraints) {
        return checked(u8(ordinal), constraints);
    }

    /**
     * An unsigned 8-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.u8()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u8(int ordinal, NumFormat format, Constraints<Integer> constraints) {
        return checked(u8(ordinal, format), constraints);
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
     * An unsigned 16-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.u16()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u16(int ordinal, Constraints<Integer> constraints) {
        return checked(u16(ordinal), constraints);
    }

    /**
     * An unsigned 16-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.u16()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u16(int ordinal, NumFormat format, Constraints<Integer> constraints) {
        return checked(u16(ordinal, format), constraints);
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
     * An unsigned 32-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.u32()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u32(int ordinal, Constraints<Long> constraints) {
        return checked(u32(ordinal), constraints);
    }

    /**
     * An unsigned 32-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.u32()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u32(int ordinal, NumFormat format, Constraints<Long> constraints) {
        return checked(u32(ordinal, format), constraints);
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
     * An unsigned 64-bit column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.u64()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u64(int ordinal, Constraints<Long> constraints) {
        return checked(u64(ordinal), constraints);
    }

    /**
     * An unsigned 64-bit column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.u64()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column u64(int ordinal, NumFormat format, Constraints<Long> constraints) {
        return checked(u64(ordinal, format), constraints);
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
     * A 32-bit floating-point column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.f32()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column f32(int ordinal, Constraints<Float> constraints) {
        return checked(f32(ordinal), constraints);
    }

    /**
     * A 32-bit floating-point column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.f32()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column f32(int ordinal, NumFormat format, Constraints<Float> constraints) {
        return checked(f32(ordinal, format), constraints);
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
     * A 64-bit floating-point column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.f64()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column f64(int ordinal, Constraints<Double> constraints) {
        return checked(f64(ordinal), constraints);
    }

    /**
     * A 64-bit floating-point column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.f64()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column f64(int ordinal, NumFormat format, Constraints<Double> constraints) {
        return checked(f64(ordinal, format), constraints);
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
     * An exact decimal column under the invariant notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.decimal()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column decimal(int ordinal, Constraints<BigDecimal> constraints) {
        return checked(decimal(ordinal), constraints);
    }

    /**
     * An exact decimal column under a declared notation, judged against {@code constraints}.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param format the declared numeric notation
     * @param constraints the constraints, built by {@code Constraints.decimal()}
     * @return the column
     * @throws IllegalArgumentException if {@code constraints} were built for another door
     */
    public static Column decimal(int ordinal, NumFormat format, Constraints<BigDecimal> constraints) {
        return checked(decimal(ordinal, format), constraints);
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
        return new Column(
                ordinal,
                Door.UNIX,
                Objects.requireNonNull(precision, "precision").code(),
                NumFormat.INVARIANT);
    }

    /**
     * An Excel date-serial column under the declared date system.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param epoch the declared date system
     * @return the column
     */
    public static Column excelSerial(int ordinal, ExcelEpoch epoch) {
        return new Column(
                ordinal,
                Door.EXCEL_SERIAL,
                Objects.requireNonNull(epoch, "epoch").code(),
                NumFormat.INVARIANT);
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
        return new Column(
                ordinal,
                Door.DATE_ORDERED,
                Objects.requireNonNull(order, "order").code(),
                NumFormat.INVARIANT);
    }

    /**
     * A zone-less civil date-time column under the declared field order.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param order the declared field order of the date part
     * @return the column
     */
    public static Column dateTime(int ordinal, DateOrder order) {
        return new Column(
                ordinal, Door.DATETIME, Objects.requireNonNull(order, "order").code(), NumFormat.INVARIANT);
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

    /**
     * A text column judged against {@code constraints}: the lengths and {@code enum}, after the
     * {@code trim} they declare. An {@code enum} match reads as the entry as declared.
     *
     * @param ordinal zero-based ordinal of the source column
     * @param constraints the constraints, built by {@code Constraints.text()}
     * @return the column
     */
    public static Column text(int ordinal, Constraints<String> constraints) {
        return checked(text(ordinal), constraints);
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Column that
                && ordinal == that.ordinal
                && door == that.door
                && declared == that.declared
                && format.equals(that.format)
                && Objects.equals(tokens, that.tokens)
                && Objects.equals(dateOptions, that.dateOptions)
                && Objects.equals(months, that.months)
                && Objects.equals(constraints, that.constraints);
    }

    @Override
    public int hashCode() {
        return Objects.hash(ordinal, door, declared, format, tokens, dateOptions, months, constraints);
    }

    @Override
    public String toString() {
        return "Column[ordinal=" + ordinal + ", door=" + door
                + (declared == 0 ? "" : ", declared=" + declared)
                + (format.equals(NumFormat.INVARIANT) ? "" : ", format=" + format)
                + (tokens == null ? "" : ", tokens=" + tokens)
                + (dateOptions == null ? "" : ", dateOptions=" + dateOptions)
                + (months == null ? "" : ", months=" + months)
                + (constraints == null ? "" : ", constraints=" + constraints) + "]";
    }
}
