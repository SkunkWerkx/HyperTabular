package io.github.skunkwerkx.hypertabular;

import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.ByteOrder;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import org.graalvm.polyglot.Context;
import org.graalvm.polyglot.Source;
import org.graalvm.polyglot.Value;
import org.graalvm.polyglot.io.ByteSequence;

/**
 * The core as a {@code wasm32-wasip1} module, run inside the JVM by
 * <a href="https://www.graalvm.org/webassembly/">GraalWasm</a>: the same fourteen exports
 * {@link Native} downcalls into, called through the polyglot API on the module bundled at
 * {@code /native/wasm32-wasip1/hypertabular.wasm}. HyperCast's Java binding has the same
 * backend for its own core; this one is its shape, carried over to a core whose calls take
 * buffers rather than scalars.
 *
 * <p><b>Memory.</b> A wasm guest sees only its own linear memory, so every buffer the core
 * reads or writes has a twin in the guest, allocated with the module's exported
 * {@code malloc} (wasi-libc's; HyperCast's account of why a host-picked offset is not safe
 * holds here too). The readers above keep working in this process's memory, exactly as on
 * the native path, and each call here mirrors what it hands the core:
 *
 * <ul>
 *   <li>What a call carries directly and that changes from call to call — the input text,
 *       the plan, the dialect, a sheet's part name, the result block, a header's names — is
 *       written into grow-only staging buffers for the call, and what the core wrote is
 *       read back out after it.
 *   <li>What persists between calls — the state block, a read's window, arena, cell table
 *       and row, each column's values and verdicts — has a lasting twin, found by the host
 *       address and size of the buffer the reader handed over. Before a call, the host
 *       copy is compared with what the twin was last known to hold and only what differs is
 *       written (a comparison of native memory is cheap; a write into the guest is the
 *       expensive direction, eight bytes per polyglot call); after it, what the core may
 *       have written is read back in bulk. So a buffer the reader grows — a new address,
 *       with the old contents copied in, as the resume protocol asks — gets a new twin that
 *       starts from exactly those contents, and the core goes on from where it stopped.
 *   <li>A workbook's container and its shared-string, table and style blocks are written
 *       once: the reader never changes them while the workbook is open.
 * </ul>
 *
 * <p>The pointers inside the structures that carry them — the column array and the buffers
 * block — are host addresses, and the guest's structures hold guest addresses at half the
 * width, so both are rebuilt here for each call: each host address is found again by the
 * size its structure gives (a column's door says its value width), and named by its twin.
 * Every other structure the core shares with this binding has the same layout on
 * {@code wasm32}, which has no pointer in it.
 *
 * <p>A twin lives as long as the memory it mirrors: each is tied to the state block it was
 * used with — or, for the container and the workbook's tables, to the container — and is
 * freed in the guest once that segment's arena has closed.
 *
 * <p><b>Threading.</b> One {@link Context} and one module instance serve the process, and
 * every call is serialized on this object's monitor, since a polyglot context permits no
 * concurrent access. The native path has no such lock.
 */
final class WasmBackend implements Backend {
    static final String RESOURCE_PATH = "/native/wasm32-wasip1/hypertabular.wasm";

    // What a consumer who selected this backend without its optional dependencies is told.
    // A compile-time constant, so Native can name it without loading this class.
    static final String GRAALWASM_MISSING = "hypertabular: the wasm backend needs GraalWasm on the "
            + "classpath — add org.graalvm.polyglot:polyglot and org.graalvm.polyglot:wasm "
            + "(the latter is a POM-type dependency)";

    private static final ByteOrder LITTLE_ENDIAN = ByteOrder.LITTLE_ENDIAN;
    // Eight bytes at a time in the order they sit in the host segment: read big-endian and
    // written big-endian, so the guest sees the same byte sequence.
    private static final ValueLayout.OfLong BE_LONG = ValueLayout.JAVA_LONG_UNALIGNED.withOrder(ByteOrder.BIG_ENDIAN);
    private static final ValueLayout.OfLong LONG = ValueLayout.JAVA_LONG_UNALIGNED;

    /** The guest's {@code ColumnBuffer}: two 32-bit pointers. */
    private static final int GUEST_BUFFER_BYTES = 8;

    /** The guest's {@code Buffers}: seven pointer-and-{@code usize} pairs, 32 bits each. */
    private static final int GUEST_BUFFERS_BYTES = 7 * 8;

    // What a buffers block's seven sizes count, in bytes per unit: window and arena bytes,
    // cells and table spans, row slots, string and style bytes.
    private static final long[] BUFFERS_UNIT = {1, 1, Native.SPAN_BYTES, Native.SLOT_BYTES, 1, Native.SPAN_BYTES, 1};

    /** The first of the buffers block's slots the reader never changes: strings, table, kinds. */
    private static final int BUFFERS_TABLES = 4;

    private final Value memory;
    private final Value mallocFn;
    private final Value freeFn;
    private final Value versionFn;
    private final Value stateSizeFn;
    private final Value initFn;
    private final Value headerFn;
    private final Value fillFn;
    private final Value unescapeFn;
    private final Value bookStateSizeFn;
    private final Value sheetFn;
    private final Value bookFillFn;
    private final Value[] bookFns = new Value[Native.Book.values().length];

    // The staging buffers, one per role a call can need at once.
    private final Staging input = new Staging();
    private final Staging aux = new Staging();
    private final Staging aux2 = new Staging();
    private final Staging out = new Staging();
    private final Staging specs = new Staging();
    private final Staging columns = new Staging();
    private final Staging buffers = new Staging();

    // The lasting twins, by host address and size.
    private final Map<Key, Twin> twins = new HashMap<>();

    WasmBackend() {
        byte[] module;
        try (InputStream in = WasmBackend.class.getResourceAsStream(RESOURCE_PATH)) {
            if (in == null) {
                throw new IllegalStateException(
                        RESOURCE_PATH + " classpath resource not found (this jar was built without the wasm module)");
            }
            module = in.readAllBytes();
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        Context context;
        try {
            // The core imports nothing from WASI today; the builtin is there so that a module
            // whose startup or panic path does (as HyperCast's does) still instantiates.
            context = Context.newBuilder("wasm")
                    .option("wasm.Builtins", "wasi_snapshot_preview1")
                    .build();
        } catch (IllegalArgumentException | IllegalStateException noWasmLanguage) {
            // The polyglot API is on the classpath but nothing behind it can run wasm: the
            // half-added dependency, org.graalvm.polyglot:polyglot without :wasm.
            throw new IllegalStateException(GRAALWASM_MISSING, noWasmLanguage);
        }
        Value exports = context.eval(Source.newBuilder("wasm", ByteSequence.create(module), "hypertabular")
                        .buildLiteral())
                .newInstance()
                .getMember("exports");
        memory = exports.getMember("memory");
        mallocFn = export(exports, "malloc");
        freeFn = export(exports, "free");
        versionFn = export(exports, "hypertabular_version");
        stateSizeFn = export(exports, "hypertabular_delimited_state_size");
        initFn = export(exports, "hypertabular_delimited_init");
        headerFn = export(exports, "hypertabular_delimited_header");
        fillFn = export(exports, "hypertabular_delimited_fill");
        unescapeFn = export(exports, "hypertabular_delimited_unescape");
        bookStateSizeFn = export(exports, "hypertabular_workbook_state_size");
        sheetFn = export(exports, "hypertabular_workbook_sheet");
        bookFillFn = export(exports, "hypertabular_workbook_fill");
        for (Native.Book call : Native.Book.values()) {
            bookFns[call.ordinal()] = export(exports, call.symbol());
        }
    }

    private static Value export(Value exports, String name) {
        Value fn = exports.getMember(name);
        if (fn == null || !fn.canExecute()) {
            throw new IllegalStateException("hypertabular: the wasm module does not export " + name);
        }
        return fn;
    }

    @Override
    public String name() {
        return "wasm";
    }

    // ---- guest memory -------------------------------------------------------------------

    private int malloc(long size) {
        int ptr = mallocFn.execute(guestSize(Math.max(size, 1))).asInt();
        if (ptr == 0) {
            throw new IllegalStateException("hypertabular: the wasm guest's malloc(" + size + ") returned NULL");
        }
        return ptr;
    }

    // A wasm32 usize: what the guest's memory can address at all.
    private static int guestSize(long size) {
        if (size < 0 || size > Integer.MAX_VALUE) {
            throw new IllegalArgumentException(
                    "hypertabular: " + size + " bytes cannot be handed to the wasm32 core in one buffer");
        }
        return (int) size;
    }

    /** Writes {@code length} bytes of {@code from}, starting at {@code at}, to the guest address {@code to + at}. */
    private void write(MemorySegment from, long at, long length, int to) {
        long i = at;
        long end = at + length;
        for (; i + 8 <= end; i += 8) {
            memory.writeBufferLong(ByteOrder.BIG_ENDIAN, to + i, from.get(BE_LONG, i));
        }
        for (; i < end; i++) {
            memory.writeBufferByte(to + i, from.get(ValueLayout.JAVA_BYTE, i));
        }
    }

    /** Reads {@code length} guest bytes at {@code from + at} into {@code shadow} and the host segment at the same offset. */
    private void read(int from, long at, long length, byte[] shadow, MemorySegment to) {
        if (length <= 0) {
            return;
        }
        int n = (int) length;
        memory.readBuffer(from + at, shadow, (int) at, n);
        MemorySegment.copy(shadow, (int) at, to, ValueLayout.JAVA_BYTE, at, n);
    }

    /** A grow-only guest buffer that a call's own arguments are written into. */
    private final class Staging {
        int ptr;
        long cap;
        byte[] readback = new byte[0];

        int reserve(long size) {
            if (size > cap) {
                if (ptr != 0) {
                    freeFn.execute(ptr);
                    ptr = 0;
                    cap = 0;
                }
                ptr = malloc(size);
                cap = size;
                readback = new byte[guestSize(size)];
            }
            return ptr;
        }

        /** The segment's first {@code size} bytes, in the guest. NULL for nothing, which the core never reads. */
        int in(MemorySegment from, long size) {
            if (size == 0 || from.equals(MemorySegment.NULL)) {
                return 0;
            }
            int at = reserve(size);
            write(from, 0, size, at);
            return at;
        }

        /** Room for {@code size} bytes the core will write; nothing is copied in. */
        int room(MemorySegment to, long size) {
            return size == 0 || to.equals(MemorySegment.NULL) ? 0 : reserve(size);
        }

        void back(MemorySegment to, long size) {
            if (size > 0 && ptr != 0) {
                read(ptr, 0, Math.min(size, cap), readback, to);
            }
        }
    }

    private record Key(long address, long size) {}

    /** A lasting guest copy of one host buffer, and what it was last known to hold. */
    private final class Twin {
        final MemorySegment host;
        // The segment whose arena decides how long this twin lives.
        final MemorySegment owner;
        final boolean fixed;
        final int guest;
        final byte[] shadow;

        Twin(MemorySegment host, MemorySegment owner, boolean fixed) {
            this.host = host;
            this.owner = owner;
            this.fixed = fixed;
            this.guest = malloc(host.byteSize());
            this.shadow = new byte[guestSize(host.byteSize())];
            write(host, 0, host.byteSize(), guest);
            MemorySegment.copy(host, ValueLayout.JAVA_BYTE, 0, shadow, 0, shadow.length);
        }

        /** Brings the guest copy up to date with the host's, writing only from the first byte that differs. */
        void in() {
            if (fixed) {
                return;
            }
            long first = host.mismatch(MemorySegment.ofArray(shadow));
            if (first < 0) {
                return;
            }
            long end = shadow.length;
            write(host, first, end - first, guest);
            MemorySegment.copy(host, ValueLayout.JAVA_BYTE, first, shadow, (int) first, (int) (end - first));
        }

        /** Reads what the core may have written back into the host buffer. */
        void out(long bytes) {
            read(guest, 0, Math.min(bytes, shadow.length), shadow, host);
        }
    }

    /**
     * The twin of a host buffer, made (and filled from the host) if there is none yet. A
     * buffer of no size, or none, is the guest's NULL.
     */
    private Twin twin(MemorySegment host, MemorySegment owner, boolean fixed) {
        if (host.byteSize() == 0 || host.address() == 0) {
            return null;
        }
        Key key = new Key(host.address(), host.byteSize());
        Twin twin = twins.get(key);
        if (twin == null) {
            twin = new Twin(host, owner, fixed);
            twins.put(key, twin);
        } else {
            twin.in();
        }
        return twin;
    }

    /** A host address handed over inside a structure, as a segment of the size the structure gives. */
    @SuppressWarnings("restricted")
    private static MemorySegment at(long address, long size) {
        return address == 0 || size == 0
                ? MemorySegment.NULL
                : MemorySegment.ofAddress(address).reinterpret(size);
    }

    private static int guest(Twin twin) {
        return twin == null ? 0 : twin.guest;
    }

    // Frees the twins of buffers whose arenas have closed. Called on every call that makes
    // twins; there are a handful per open reader, so the walk costs nothing worth keeping.
    private void sweep() {
        Iterator<Twin> each = twins.values().iterator();
        while (each.hasNext()) {
            Twin twin = each.next();
            if (!twin.owner.scope().isAlive()) {
                freeFn.execute(twin.guest);
                each.remove();
            }
        }
    }

    // ---- the structures that carry pointers ---------------------------------------------

    /** The column array, rebuilt with each column's values and verdicts named by their twins. */
    private int columns(
            MemorySegment specsIn,
            MemorySegment hostColumns,
            long count,
            long maxRows,
            MemorySegment owner,
            List<Twin> values,
            List<Twin> verdicts,
            long[] widths) {
        if (count == 0) {
            return 0;
        }
        int array = columns.reserve(count * GUEST_BUFFER_BYTES);
        MemorySegment entry = hostColumns.asSlice(0, count * Native.BUFFER_BYTES);
        for (int i = 0; i < count; i++) {
            int code = specsIn.get(ValueLayout.JAVA_INT, i * Native.SPEC_BYTES + Native.SPEC_DOOR);
            long width = valueBytes(code);
            widths[i] = width;
            Twin value =
                    twin(at(entry.get(ValueLayout.JAVA_LONG, i * Native.BUFFER_BYTES), maxRows * width), owner, false);
            Twin verdict = twin(
                    at(
                            entry.get(ValueLayout.JAVA_LONG, i * Native.BUFFER_BYTES + Native.BUFFER_VERDICTS),
                            maxRows * Native.VERDICT_BYTES),
                    owner,
                    false);
            values.add(value);
            verdicts.add(verdict);
            memory.writeBufferInt(LITTLE_ENDIAN, array + i * GUEST_BUFFER_BYTES, guest(value));
            memory.writeBufferInt(LITTLE_ENDIAN, array + i * GUEST_BUFFER_BYTES + 4, guest(verdict));
        }
        return array;
    }

    private static long valueBytes(int code) {
        for (Door door : Door.values()) {
            if (door.code() == code) {
                return door.valueBytes();
            }
        }
        // A plan is checked before it is ever handed over, so this is a broken invariant.
        throw new IllegalStateException("hypertabular: no door has code " + code);
    }

    /**
     * The buffers block, rebuilt with each buffer named by its twin. The scratch twins are
     * tied to the state block, the workbook's tables (never changed while it is open) to
     * the container.
     */
    private int buffers(MemorySegment hostBuffers, MemorySegment state, MemorySegment container, Twin[] twins) {
        MemorySegment block = hostBuffers.asSlice(0, Native.BUFFERS_BYTES);
        int guestBlock = buffers.reserve(GUEST_BUFFERS_BYTES);
        for (int slot = 0; slot < 7; slot++) {
            long address = block.get(ValueLayout.JAVA_LONG, slot * 16L);
            long count = block.get(ValueLayout.JAVA_LONG, slot * 16L + 8);
            boolean table = slot >= BUFFERS_TABLES;
            MemorySegment owner = table && container != null ? container : state;
            Twin twin = twin(at(address, count * BUFFERS_UNIT[slot]), owner, table);
            twins[slot] = twin;
            memory.writeBufferInt(LITTLE_ENDIAN, guestBlock + slot * 8, guest(twin));
            memory.writeBufferInt(LITTLE_ENDIAN, guestBlock + slot * 8 + 4, twin == null ? 0 : guestSize(count));
        }
        return guestBlock;
    }

    /** Reads back the scratch the core may have written: window, arena, cell table and row. */
    private static void scratchOut(Twin[] twins) {
        for (int slot = 0; slot < BUFFERS_TABLES; slot++) {
            if (twins[slot] != null) {
                twins[slot].out(Long.MAX_VALUE);
            }
        }
    }

    /** Reads back the rows the core filled in each column. */
    private void columnsOut(MemorySegment filled, List<Twin> values, List<Twin> verdicts, long[] widths) {
        long rows = filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ROWS);
        for (int i = 0; i < values.size(); i++) {
            if (values.get(i) != null) {
                values.get(i).out(rows * widths[i]);
            }
            if (verdicts.get(i) != null) {
                verdicts.get(i).out(rows * Native.VERDICT_BYTES);
            }
        }
    }

    // ---- the exports ----------------------------------------------------------------------

    @Override
    public synchronized int version() {
        return versionFn.execute().asInt();
    }

    @Override
    public synchronized long stateSize() {
        return Integer.toUnsignedLong(stateSizeFn.execute().asInt());
    }

    @Override
    public synchronized long workbookStateSize() {
        return Integer.toUnsignedLong(bookStateSizeFn.execute().asInt());
    }

    @Override
    public synchronized int init(MemorySegment state, MemorySegment dialect) {
        sweep();
        Twin st = twin(state, state, false);
        int code =
                initFn.execute(guest(st), aux.in(dialect, Native.DIALECT_BYTES)).asInt();
        st.out(Long.MAX_VALUE);
        return code;
    }

    @Override
    public synchronized int header(
            MemorySegment state,
            MemorySegment inputText,
            long inputLen,
            boolean last,
            MemorySegment names,
            long namesCap,
            MemorySegment arena,
            long arenaCap,
            MemorySegment result) {
        sweep();
        Twin st = twin(state, state, false);
        int code = headerFn.execute(
                        guest(st),
                        input.in(inputText, inputLen),
                        guestSize(inputLen),
                        last ? 1 : 0,
                        aux.room(names, namesCap * Native.SPAN_BYTES),
                        guestSize(namesCap),
                        aux2.room(arena, arenaCap),
                        guestSize(arenaCap),
                        out.room(result, Native.FILLED_BYTES))
                .asInt();
        st.out(Long.MAX_VALUE);
        out.back(result, Native.FILLED_BYTES);
        aux.back(names, namesCap * Native.SPAN_BYTES);
        aux2.back(arena, arenaCap);
        return code;
    }

    @Override
    public synchronized int fill(
            MemorySegment state,
            MemorySegment inputText,
            long inputLen,
            boolean last,
            MemorySegment plan,
            MemorySegment hostColumns,
            long columnCount,
            long maxRows,
            MemorySegment hostBuffers,
            MemorySegment result) {
        sweep();
        Twin st = twin(state, state, false);
        MemorySegment planIn = plan.asSlice(0, columnCount * Native.SPEC_BYTES);
        List<Twin> values = new ArrayList<>();
        List<Twin> verdicts = new ArrayList<>();
        long[] widths = new long[(int) columnCount];
        Twin[] scratch = new Twin[7];
        int code = fillFn.execute(
                        guest(st),
                        input.in(inputText, inputLen),
                        guestSize(inputLen),
                        last ? 1 : 0,
                        specs.in(planIn, columnCount * Native.SPEC_BYTES),
                        columns(planIn, hostColumns, columnCount, maxRows, state, values, verdicts, widths),
                        guestSize(columnCount),
                        guestSize(maxRows),
                        buffers(hostBuffers, state, null, scratch),
                        out.room(result, Native.FILLED_BYTES))
                .asInt();
        st.out(Long.MAX_VALUE);
        out.back(result, Native.FILLED_BYTES);
        scratchOut(scratch);
        columnsOut(result, values, verdicts, widths);
        return code;
    }

    @Override
    public synchronized long unescape(MemorySegment cell, long len, MemorySegment result, long cap) {
        long written = Integer.toUnsignedLong(unescapeFn
                .execute(input.in(cell, len), guestSize(len), aux.room(result, cap), guestSize(cap))
                .asInt());
        aux.back(result, written);
        return written;
    }

    @Override
    public synchronized int book(
            Native.Book call,
            MemorySegment state,
            MemorySegment container,
            MemorySegment hostBuffers,
            MemorySegment result) {
        sweep();
        Twin st = twin(state, state, false);
        Twin book = twin(container, container, true);
        Twin[] scratch = new Twin[7];
        int code = bookFns[call.ordinal()]
                .execute(
                        guest(st),
                        guest(book),
                        guestSize(container.byteSize()),
                        buffers(hostBuffers, state, container, scratch),
                        out.room(result, Native.FILLED_BYTES))
                .asInt();
        st.out(Long.MAX_VALUE);
        out.back(result, Native.FILLED_BYTES);
        scratchOut(scratch);
        return code;
    }

    @Override
    public synchronized int sheet(
            MemorySegment state,
            MemorySegment container,
            MemorySegment part,
            int index,
            boolean hasHeader,
            boolean skipEmptyRows,
            MemorySegment result) {
        sweep();
        Twin st = twin(state, state, false);
        Twin book = twin(container, container, true);
        int code = sheetFn.execute(
                        guest(st),
                        guest(book),
                        guestSize(container.byteSize()),
                        aux.in(part, part.byteSize()),
                        guestSize(part.byteSize()),
                        index,
                        hasHeader ? 1 : 0,
                        skipEmptyRows ? 1 : 0,
                        out.room(result, Native.FILLED_BYTES))
                .asInt();
        st.out(Long.MAX_VALUE);
        out.back(result, Native.FILLED_BYTES);
        return code;
    }

    @Override
    public synchronized int bookFill(
            MemorySegment state,
            MemorySegment container,
            MemorySegment plan,
            MemorySegment hostColumns,
            long columnCount,
            long maxRows,
            MemorySegment hostBuffers,
            MemorySegment result) {
        sweep();
        Twin st = twin(state, state, false);
        Twin book = twin(container, container, true);
        MemorySegment planIn = plan.asSlice(0, columnCount * Native.SPEC_BYTES);
        List<Twin> values = new ArrayList<>();
        List<Twin> verdicts = new ArrayList<>();
        long[] widths = new long[(int) columnCount];
        Twin[] scratch = new Twin[7];
        int code = bookFillFn
                .execute(
                        guest(st),
                        guest(book),
                        guestSize(container.byteSize()),
                        specs.in(planIn, columnCount * Native.SPEC_BYTES),
                        columns(planIn, hostColumns, columnCount, maxRows, state, values, verdicts, widths),
                        guestSize(columnCount),
                        guestSize(maxRows),
                        buffers(hostBuffers, state, container, scratch),
                        out.room(result, Native.FILLED_BYTES))
                .asInt();
        st.out(Long.MAX_VALUE);
        out.back(result, Native.FILLED_BYTES);
        scratchOut(scratch);
        columnsOut(result, values, verdicts, widths);
        return code;
    }
}
