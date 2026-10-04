package io.github.skunkwerkx.hypertabular;

/**
 * The door a column is cast through: HyperCast's twenty-one, plus {@link #TEXT} for the
 * bytes themselves. Named as the native ABI and HyperCast's own {@code Cast} doors name
 * them, and each presented as the Java type {@code Cast} presents the same door as.
 */
public enum Door {
    /** {@link Boolean}, HyperCast's boolean lexicon. */
    BOOL(1, 1),
    /** A signed 8-bit integer, as {@link Byte}. */
    I8(2, 1),
    /** A signed 16-bit integer, as {@link Short}. */
    I16(3, 2),
    /** A signed 32-bit integer, as {@link Integer}. */
    I32(4, 4),
    /** A signed 64-bit integer, as {@link Long}. */
    I64(5, 8),
    /** An unsigned 8-bit integer, widened to {@link Integer}. */
    U8(6, 1),
    /** An unsigned 16-bit integer, widened to {@link Integer}. */
    U16(7, 2),
    /** An unsigned 32-bit integer, widened to {@link Long}. */
    U32(8, 4),
    /**
     * An unsigned 64-bit integer, as the {@link Long} with the same bits — read it with
     * {@link Long#toUnsignedString(long)} and the other {@code Long} unsigned methods.
     */
    U64(9, 8),
    /** {@link Float}. */
    F32(10, 4),
    /** {@link Double}. */
    F64(11, 8),
    /** {@link java.util.UUID}. */
    UUID(12, 16),
    /** An RFC 3339 instant, as {@link java.time.Instant}. */
    TIMESTAMP(13, 16),
    /** A Unix-epoch integer at a declared precision, as {@link java.time.Instant}. */
    UNIX(14, 16),
    /** A strict {@code yyyy-MM-dd} date, as {@link java.time.LocalDate}. */
    DATE(15, 4),
    /** A 24-hour time of day, as {@link java.time.LocalTime}. */
    TIME(16, 8),
    /** A duration, as {@link java.time.Duration}. */
    DURATION(17, 16),
    /** The cell's bytes themselves — no cast. */
    TEXT(18, 8),
    /** An exact {@link java.math.BigDecimal}; no float is ever formed. */
    DECIMAL(19, 16),
    /** A separated calendar date under a declared field order, as {@link java.time.LocalDate}. */
    DATE_ORDERED(20, 4),
    /** A zone-less civil date-time under a declared field order, as {@link java.time.LocalDateTime}. */
    DATETIME(21, 16),
    /** An Excel date serial under a declared date system, as {@link java.time.Instant}. */
    EXCEL_SERIAL(22, 16);

    private final int code;
    private final int valueBytes;

    Door(int code, int valueBytes) {
        this.code = code;
        this.valueBytes = valueBytes;
    }

    /**
     * The native core's discriminant for this door ({@code 0} is never a door).
     *
     * @return the door's ABI code
     */
    public int code() {
        return code;
    }

    /** Bytes one value of this door takes in a column buffer. */
    int valueBytes() {
        return valueBytes;
    }

    /** True for the doors whose column buffer is an array of one Java primitive. */
    boolean isPrimitive() {
        return code <= F64.code;
    }
}
