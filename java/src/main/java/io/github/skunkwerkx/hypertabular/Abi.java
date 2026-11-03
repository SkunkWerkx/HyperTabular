package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.lang.foreign.ValueLayout;

/**
 * The native core's C ABI as data: the return codes, and the sizes and offsets of the
 * {@code #[repr(C)]} shapes that cross it ({@code rust/src/kernel/abi.rs},
 * {@code rust/src/kernel/exports.rs}). Both paths to the core read these: {@link Native}, the
 * JNI base, and its JDK 25 namesake, which reaches the same exports through FFM.
 */
final class Abi {
    private Abi() {}

    /** The call did what it could; the result says how far it got. */
    static final int OK = 0;
    /** The data is structurally broken; the result's failure says where. */
    static final int ERR_STRUCTURE = -2;
    /** The arena cannot hold what one row needs; the result says how much that is. */
    static final int ERR_ARENA = -3;
    /** The cell table cannot hold one row; the result says how many entries one takes. */
    static final int ERR_CELLS = -4;
    /** A workbook part's window cannot hold the token being read; the result says what would. */
    static final int ERR_WINDOW = -5;

    /** {@code Span}: {@code {u32 offset, u32 len}}, the top bit of {@code len} a flag. */
    static final long SPAN_BYTES = 8;
    /** The length bits of a span's {@code len}; the flag is the sign bit of the Java int. */
    static final int SPAN_LENGTH = Integer.MAX_VALUE;

    /** {@code CellVerdict}: {@code {u32 offset, u32 len, u32 reason}}. */
    static final long VERDICT_BYTES = 12;

    /**
     * {@code ColumnSpec}: {@code {u32 ordinal, u32 door, u32 param, RawNumFormat format}},
     * the format HyperCast's 92-byte size-tagged layout ({@link NativeValues#FORMAT_BYTES}:
     * a size word, the flags, the native digits' zero, then every symbol inline with its
     * length), all zeros for the invariant notation.
     */
    static final long SPEC_BYTES = 12 + NativeValues.FORMAT_BYTES;

    static final long SPEC_ORDINAL = 0;
    static final long SPEC_DOOR = 4;
    static final long SPEC_PARAM = 8;
    /** Where the format sits in a spec: HyperCast's {@link NativeValues#FORMAT_BYTES} of it. */
    static final long SPEC_FORMAT = 12;

    /** {@code ColumnBuffer}: {@code {void* values, CellVerdict* verdicts}}. */
    static final long BUFFER_BYTES = 2 * ValueLayout.ADDRESS.byteSize();

    static final long BUFFER_VERDICTS = ValueLayout.ADDRESS.byteSize();

    /** {@code RawDialect}: {@code {u8 separator, u8 quoting, u8 skip_blank_lines, u8 engine}}. */
    static final long DIALECT_BYTES = 4;

    /**
     * {@code Filled}: {@code {u64 rows, u64 consumed, u64 arena_used, u64 needed,
     * Failure failure}}, the failure {@code {u32 code, u32 line, u64 record, u64 byte,
     * u32 expected, u32 found}}.
     */
    static final long FILLED_BYTES = 64;

    static final long FILLED_ROWS = 0;
    static final long FILLED_CONSUMED = 8;
    static final long FILLED_ARENA_USED = 16;
    static final long FILLED_NEEDED = 24;
    static final long FAILURE_CODE = 32;
    static final long FAILURE_LINE = 36;
    static final long FAILURE_RECORD = 40;
    static final long FAILURE_BYTE = 48;
    static final long FAILURE_EXPECTED = 56;
    static final long FAILURE_FOUND = 60;

    /**
     * {@code Opened}: {@code {u32 format, u32 epoch, u64 strings_bytes, u64 strings_count,
     * u64 needed, Failure failure}} — the failure at the same offset as in {@code Filled}.
     */
    static final long OPENED_BYTES = 64;

    static final long OPENED_FORMAT = 0;
    static final long OPENED_EPOCH = 4;
    static final long OPENED_STRINGS_BYTES = 8;
    static final long OPENED_NEEDED = 24;

    /** {@code Slot}: {@code {u32 tag, u32 aux, u64 bits}}, the row a workbook read assembles. */
    static final long SLOT_BYTES = 16;

    /**
     * {@code Buffers}: seven pointer-and-size pairs — {@code window}, {@code arena},
     * {@code cells}, {@code row}, then the workbook's {@code strings}, {@code table} and
     * {@code kinds} — each a pointer and a {@code usize}.
     */
    static final long BUFFERS_BYTES = 14 * 8;

    static final long BUFFERS_WINDOW = 0;
    static final long BUFFERS_ARENA = 16;
    static final long BUFFERS_CELLS = 32;
    static final long BUFFERS_ROW = 48;
    static final long BUFFERS_STRINGS = 64;
    static final long BUFFERS_TABLE = 80;
    static final long BUFFERS_KINDS = 96;

    /** The window a workbook part is inflated through starts at the size the core requires. */
    static final long WINDOW_MIN = 64 * 1024;

    /**
     * The fields of the state block this binding reads: {@code line} (u32), {@code records}
     * and {@code offset} (u64), and {@code expected} (u32, the cells a record has; 0 until
     * known). The rest is the core's; the block's size is asked of the
     * library ({@link Native#stateSize()}), never assumed.
     */
    static final long STATE_LINE = 4;

    static final long STATE_RECORDS = 8;
    static final long STATE_OFFSET = 16;
    static final long STATE_EXPECTED = 24;
    /** The bytes of the state block the offsets above reach into. */
    static final long STATE_READ_BYTES = 28;

    /** The workbook calls that share one shape: which export is the caller's to name. */
    enum Book {
        OPEN("hypertabular_workbook_open"),
        SHEETS("hypertabular_workbook_sheets"),
        STRINGS("hypertabular_workbook_strings"),
        STYLES("hypertabular_workbook_styles"),
        HEADER("hypertabular_workbook_header");

        /** The export's symbol, for a diagnostic. */
        final String symbol;

        Book(String symbol) {
            this.symbol = symbol;
        }
    }
}
