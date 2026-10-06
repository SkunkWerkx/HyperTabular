//go:build cgo && !tinygo && (darwin || linux || windows) && (amd64 || arm64)

// The native backend: libhypertabular linked into the binary, as HyperCast's Go module links
// libhypercast. The core is a static library under staticlib/{goos}_{goarch}/, named on the
// cgo link line below, and its six exports are ordinary C calls to symbols the linker
// resolved. Nothing is embedded, nothing is written to a temp directory, nothing is loaded at
// run time. That takes cgo, and so a C compiler wherever the module is built; anything else
// does not compile, and unsupported.go and unsupported_tinygo.go say so by name.
//
// Both cores end up in one binary — this module's archive and HyperCast's, which the
// hypercast package this one imports links for itself. They were built to allow it: each
// archive is the crate's one object with no copy of Rust's standard library, and this one
// is built with HyperCast's `exports` off, so it carries none of HyperCast's cast_* symbols
// and the two define nothing in common.
//
// The archives are per platform exactly as HyperCast's are (its backend_static.go says
// why): the two Linux ones are the musl builds, linked on glibc and musl alike, and the two
// Windows ones are the MSVC builds, which MinGW's linker reads.
//
// The Windows archives Go links leave out Rust's own 128-bit division helpers (__divti3,
// __udivti3), which the C# and Swift copies of the same MSVC build keep because MSVC has
// none: MinGW supplies them — libgcc on amd64, compiler-rt on arm64 — and Rust emits them
// as COFF weak externals that GNU ld, finding them in both cores' archives, refused as a
// multiple definition. The forge's build-static-libs.sh says the rest.
//
// # What crosses, and the cgo pointer rules
//
// One call per batch: ht_fill below for delimited text, ht_book for a workbook. Everything
// they are handed is Go memory, and the rules for
// passing Go pointers to C (cmd/cgo, "Passing pointers") are met by construction rather
// than by pinning:
//
//   - Every pointer argument points to memory with no Go pointer in it — bytes, spans,
//     specs, the core's state block, the block of column buffers. Such a pointer may be
//     passed as it is, and what it points to is pinned for the duration of the call.
//   - The core's Buffers block, which a workbook call is handed, also holds pointers — to
//     the scratch the call works in and the workbook's tables. It too is assembled in C
//     memory, on ht_book's stack, from pointers Go passes as plain arguments.
//   - The core's ColumnBuffer array is the other shape that holds pointers: per column, where
//     its values go and where its verdicts go. As Go memory it would be Go pointers inside
//     Go memory handed to C, which is only allowed when every one of them is pinned — a
//     runtime.Pinner over two arrays per column, held for the reader's life (and a leak
//     panic from the pinner's finalizer for a reader nobody closed) or re-pinned on every
//     batch. Instead the array never exists on the Go side. Every column's two arrays live
//     in one Go allocation (the reader's block); Go passes that block's address and, per
//     column, two offsets into it, and ht_fill builds the ColumnBuffer array in C memory —
//     its own stack, or the C heap for a plan too wide for that — from the one pointer it
//     was given. C may hold Go pointers in C memory for as long as the call lasts, and this
//     array does not outlive it.
//   - The core keeps nothing: no pointer it is handed is stored anywhere that survives the
//     call (rust/src/kernel/exports.rs).
//
// So nothing is pinned, nothing is allocated per batch, and the buffers are ordinary
// garbage-collected Go memory: a slice a caller kept past Close is stale, never dangling.
//
// # The archives
//
// The hypertabular_local build tag links the archive .github/scripts/local-core.sh builds
// from the checkout instead, under rust/target/ (which git ignores), so the suite can run
// against the core as it stands without replacing a committed archive:
//
//	.github/scripts/local-core.sh && (cd go && go test -tags hypertabular_local ./...)
//
// staticlib/{goos}_{goarch}/libhypertabular.a is what a build links, and git ignores it
// (.gitignore, as HyperCast's does): a locally built archive must not ride into a commit,
// and only a staging workflow that has verified build provenance adds them, with
// `git add -f`. On a checkout without one for your platform — or after changing rust/ —
// build it with the forge's script, from the repository root, beside a checkout of
// SkunkWerkx/.github:
//
//	../.github/.github/actions/static-libs/build-static-libs.sh hypertabular HyperTabular . x86_64-unknown-linux-musl
//
// naming the Rust target for your platform (aarch64-unknown-linux-musl, x86_64-apple-darwin,
// aarch64-apple-darwin, x86_64-pc-windows-msvc, aarch64-pc-windows-msvc); with none it
// builds every one. CI does exactly that on each leg before it runs this suite.

package hypertabular

/*
#cgo linux,amd64,!hypertabular_local LDFLAGS: ${SRCDIR}/staticlib/linux_amd64/libhypertabular.a
#cgo linux,arm64,!hypertabular_local LDFLAGS: ${SRCDIR}/staticlib/linux_arm64/libhypertabular.a
#cgo darwin,amd64,!hypertabular_local LDFLAGS: ${SRCDIR}/staticlib/darwin_amd64/libhypertabular.a
#cgo darwin,arm64,!hypertabular_local LDFLAGS: ${SRCDIR}/staticlib/darwin_arm64/libhypertabular.a
#cgo windows,amd64,!hypertabular_local LDFLAGS: ${SRCDIR}/staticlib/windows_amd64/libhypertabular.a
#cgo windows,arm64,!hypertabular_local LDFLAGS: ${SRCDIR}/staticlib/windows_arm64/libhypertabular.a
#cgo linux,amd64,hypertabular_local LDFLAGS: ${SRCDIR}/../rust/target/local-core/go/staticlib/linux_amd64/libhypertabular.a
#cgo linux,arm64,hypertabular_local LDFLAGS: ${SRCDIR}/../rust/target/local-core/go/staticlib/linux_arm64/libhypertabular.a
#cgo darwin,amd64,hypertabular_local LDFLAGS: ${SRCDIR}/../rust/target/local-core/go/staticlib/darwin_amd64/libhypertabular.a
#cgo darwin,arm64,hypertabular_local LDFLAGS: ${SRCDIR}/../rust/target/local-core/go/staticlib/darwin_arm64/libhypertabular.a
#cgo windows,amd64,hypertabular_local LDFLAGS: ${SRCDIR}/../rust/target/local-core/go/staticlib/windows_amd64/libhypertabular.a
#cgo windows,arm64,hypertabular_local LDFLAGS: ${SRCDIR}/../rust/target/local-core/go/staticlib/windows_arm64/libhypertabular.a
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

// The shapes that cross the C ABI — rust/src/kernel/abi.rs and
// rust/src/kernel/delimited/fill.rs, field for field.
typedef struct { uint32_t offset; uint32_t len; } ht_span;
typedef struct { uint32_t offset; uint32_t len; uint32_t reason; } ht_verdict;
// HyperCast's RawNumFormat, 32 bytes: the currency symbol is currency_len UTF-8 bytes inline.
typedef struct { uint32_t decimal_sep; uint32_t group_sep; uint32_t flags; uint32_t currency_len; uint8_t currency[16]; } ht_format;
typedef struct { uint32_t ordinal; uint32_t door; uint32_t param; ht_format format; } ht_spec;
typedef struct { void *values; ht_verdict *verdicts; } ht_buffer;
typedef struct { uint32_t code; uint32_t line; uint64_t record; uint64_t byte; uint32_t expected; uint32_t found; } ht_failure;
typedef struct { uint64_t rows; uint64_t consumed; uint64_t arena_used; uint64_t needed; ht_failure failure; } ht_filled;
typedef struct { uint8_t separator; uint8_t quoting; uint8_t skip_blank_lines; uint8_t engine; } ht_dialect;
// The state block: 64 bytes the core defines, 8-byte aligned. Go mirrors its fields
// (tabular.go's rawState) to read the position out of it; here it is only room.
typedef struct { uint64_t words[8]; } ht_state;

_Static_assert(sizeof(ht_span) == 8, "Span");
_Static_assert(sizeof(ht_verdict) == 12, "CellVerdict");
_Static_assert(sizeof(ht_spec) == 44, "ColumnSpec");
_Static_assert(sizeof(ht_failure) == 32, "Failure");
_Static_assert(sizeof(ht_filled) == 64, "Filled");
_Static_assert(sizeof(ht_dialect) == 4, "RawDialect");

// The workbook shapes (rust/src/kernel/abi.rs).
typedef struct { uint32_t tag; uint32_t aux; uint64_t bits; } ht_slot;
typedef struct {
	uint8_t *window; size_t window_cap;
	uint8_t *arena; size_t arena_cap;
	ht_span *cells; size_t cells_cap;
	ht_slot *row; size_t row_cap;
	const uint8_t *strings; size_t strings_len;
	const ht_span *table; size_t table_len;
	const uint8_t *kinds; size_t kinds_len;
} ht_buffers;
typedef struct { uint32_t format; uint32_t epoch; uint64_t strings_bytes; uint64_t strings_count; uint64_t needed; ht_failure failure; } ht_opened;

_Static_assert(sizeof(ht_slot) == 16, "Slot");
_Static_assert(sizeof(ht_buffers) == 112, "Buffers");
_Static_assert(sizeof(ht_opened) == 64, "Opened");

// The core's C ABI — rust/src/kernel/exports.rs: six exports for delimited text, eight for
// workbooks.
uint32_t hypertabular_version(void);
size_t hypertabular_delimited_state_size(void);
int32_t hypertabular_delimited_init(ht_state *state, const ht_dialect *dialect);
int32_t hypertabular_delimited_header(ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
                                      ht_span *names, size_t names_cap, uint8_t *arena, size_t arena_cap, ht_filled *out);
int32_t hypertabular_delimited_fill(ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
                                    const ht_spec *specs, const ht_buffer *columns, size_t column_count, size_t max_rows,
                                    ht_span *cells, size_t cells_cap, uint8_t *arena, size_t arena_cap, ht_filled *out);
size_t hypertabular_delimited_unescape(const uint8_t *cell, size_t len, uint8_t *out, size_t cap);
size_t hypertabular_workbook_state_size(void);
int32_t hypertabular_workbook_open(uint64_t *state, const uint8_t *container, size_t container_len,
                                   const ht_buffers *buffers, ht_opened *out);
int32_t hypertabular_workbook_sheets(uint64_t *state, const uint8_t *container, size_t container_len,
                                     const ht_buffers *buffers, ht_filled *out);
int32_t hypertabular_workbook_strings(uint64_t *state, const uint8_t *container, size_t container_len,
                                      const ht_buffers *buffers, ht_filled *out);
int32_t hypertabular_workbook_styles(uint64_t *state, const uint8_t *container, size_t container_len,
                                     const ht_buffers *buffers, ht_filled *out);
int32_t hypertabular_workbook_sheet(uint64_t *state, const uint8_t *container, size_t container_len,
                                    const uint8_t *part, size_t part_len, uint32_t index, uint32_t has_header,
                                    uint32_t skip_empty_rows, ht_filled *out);
int32_t hypertabular_workbook_header(uint64_t *state, const uint8_t *container, size_t container_len,
                                     const ht_buffers *buffers, ht_filled *out);
int32_t hypertabular_workbook_fill(uint64_t *state, const uint8_t *container, size_t container_len,
                                   const ht_spec *specs, const ht_buffer *columns, size_t column_count, size_t max_rows,
                                   const ht_buffers *buffers, ht_filled *out);

// What the shims add to the core's return codes (tabular.go's errStateSize, errShimMemory).
#define HT_ERR_STATE_SIZE (-100)
#define HT_ERR_SHIM_MEMORY (-101)

// Starts an input. The dialect is assembled here, on the C stack, so that no Go pointer to
// a local crosses; and the linked core's state block is held to the size Go mirrors before
// the core is let near it.
static int32_t ht_init(ht_state *state, uint8_t separator, uint8_t quoting, uint8_t skip_blank_lines) {
	if (hypertabular_delimited_state_size() != sizeof(ht_state)) {
		return HT_ERR_STATE_SIZE;
	}
	ht_dialect dialect = {separator, quoting, skip_blank_lines, 0};
	return hypertabular_delimited_init(state, &dialect);
}

// A plan this wide or narrower has its ColumnBuffer array on the stack: 1 KiB of it.
#define HT_STACK_COLUMNS 64

// The ColumnBuffer array a fill reads from: column i's values at block + offsets[2i] and
// its verdicts at block + offsets[2i + 1], built in C memory that does not outlive the call
// — on_stack for a plan of up to HT_STACK_COLUMNS, the C heap past that (NULL if that
// fails). See the note at the top of this file.
static ht_buffer *ht_columns(ht_buffer *on_stack, uint8_t *block, const size_t *offsets, size_t column_count) {
	ht_buffer *columns = on_stack;
	if (column_count > HT_STACK_COLUMNS) {
		columns = (ht_buffer *)malloc(column_count * sizeof(ht_buffer));
		if (columns == NULL) {
			return NULL;
		}
	}
	for (size_t i = 0; i < column_count; i++) {
		columns[i].values = block + offsets[2 * i];
		columns[i].verdicts = (ht_verdict *)(block + offsets[2 * i + 1]);
	}
	return columns;
}

// One batch of delimited text.
static int32_t ht_fill(ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
                       const ht_spec *specs, uint8_t *block, const size_t *offsets, size_t column_count, size_t max_rows,
                       ht_span *cells, size_t cells_cap, uint8_t *arena, size_t arena_cap, ht_filled *out) {
	ht_buffer on_stack[HT_STACK_COLUMNS];
	ht_buffer *columns = ht_columns(on_stack, block, offsets, column_count);
	if (columns == NULL) {
		return HT_ERR_SHIM_MEMORY;
	}
	int32_t code = hypertabular_delimited_fill(state, input, input_len, last, specs, columns, column_count, max_rows,
	                                           cells, cells_cap, arena, arena_cap, out);
	if (columns != on_stack) {
		free(columns);
	}
	return code;
}

// The workbook calls that take a Buffers block (workbook.go's bookCall).
enum { HT_OPEN, HT_SHEETS, HT_STRINGS, HT_STYLES, HT_HEADER, HT_FILL };

// One workbook call. The Buffers block is assembled here, on the C stack, from the pointers
// and sizes Go passes one by one; out is an ht_opened for HT_OPEN and an ht_filled for the
// rest. The plan (specs, block, offsets, column_count, max_rows) is read by HT_FILL alone.
static int32_t ht_book(int call, uint64_t *state, const uint8_t *container, size_t container_len,
                       uint8_t *window, size_t window_cap, uint8_t *arena, size_t arena_cap,
                       ht_span *cells, size_t cells_cap, ht_slot *row, size_t row_cap,
                       const uint8_t *strings, size_t strings_len, const ht_span *table, size_t table_len,
                       const uint8_t *kinds, size_t kinds_len,
                       const ht_spec *specs, uint8_t *block, const size_t *offsets, size_t column_count, size_t max_rows,
                       void *out) {
	ht_buffers buffers = {window, window_cap, arena,  arena_cap, cells, cells_cap, row,
	                      row_cap, strings, strings_len, table, table_len, kinds, kinds_len};
	switch (call) {
	case HT_OPEN:
		return hypertabular_workbook_open(state, container, container_len, &buffers, (ht_opened *)out);
	case HT_SHEETS:
		return hypertabular_workbook_sheets(state, container, container_len, &buffers, (ht_filled *)out);
	case HT_STRINGS:
		return hypertabular_workbook_strings(state, container, container_len, &buffers, (ht_filled *)out);
	case HT_STYLES:
		return hypertabular_workbook_styles(state, container, container_len, &buffers, (ht_filled *)out);
	case HT_HEADER:
		return hypertabular_workbook_header(state, container, container_len, &buffers, (ht_filled *)out);
	case HT_FILL: {
		ht_buffer on_stack[HT_STACK_COLUMNS];
		ht_buffer *columns = ht_columns(on_stack, block, offsets, column_count);
		if (columns == NULL) {
			return HT_ERR_SHIM_MEMORY;
		}
		int32_t code = hypertabular_workbook_fill(state, container, container_len, specs, columns, column_count,
		                                          max_rows, &buffers, (ht_filled *)out);
		if (columns != on_stack) {
			free(columns);
		}
		return code;
	}
	default:
		return -1;
	}
}
*/
import "C"

import "unsafe"

// size_t is the width of the offsets the reader keeps as uintptr.
var _ [unsafe.Sizeof(C.size_t(0)) - unsafe.Sizeof(uintptr(0))]struct{}
var _ [unsafe.Sizeof(uintptr(0)) - unsafe.Sizeof(C.size_t(0))]struct{}

// packedVersion is the core's own version, major<<16 | minor<<8 | patch, read through the
// ABI rather than from this module, so NativeVersion reports the archive that was linked.
func packedVersion() uint32 {
	return uint32(C.hypertabular_version())
}

// bytesPtr is the address of a slice's first byte for C, and nil for an empty one: the core
// never dereferences a pointer whose length is zero, and an empty slice's own address may
// be one past the end of something.
func bytesPtr(bytes []byte) *C.uint8_t {
	if len(bytes) == 0 {
		return nil
	}
	return (*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(bytes)))
}

func spansPtr(spans []rawSpan) *C.ht_span {
	if len(spans) == 0 {
		return nil
	}
	return (*C.ht_span)(unsafe.Pointer(unsafe.SliceData(spans)))
}

func flag(set bool) C.uint8_t {
	if set {
		return 1
	}
	return 0
}

// nativeInit writes a fresh state for the dialect into the reader's state block.
func (r *DelimitedReader) nativeInit(dialect Dialect) int32 {
	return int32(C.ht_init((*C.ht_state)(unsafe.Pointer(&r.core.state)),
		C.uint8_t(dialect.Separator), flag(dialect.Quoting), flag(dialect.SkipBlankLines)))
}

// nativeHeader reads the next record of window as a header into names; the result is in
// r.core.filled.
func (r *DelimitedReader) nativeHeader(window []byte, last bool, names []rawSpan) int32 {
	return int32(C.hypertabular_delimited_header(
		(*C.ht_state)(unsafe.Pointer(&r.core.state)),
		bytesPtr(window), C.size_t(len(window)), C.uint32_t(flag(last)),
		spansPtr(names), C.size_t(len(names)),
		bytesPtr(r.arena), C.size_t(len(r.arena)),
		(*C.ht_filled)(unsafe.Pointer(&r.core.filled))))
}

// nativeFill reads up to maxRows whole rows of window into the reader's column buffers —
// the one crossing a batch makes; the result is in r.core.filled.
func (r *DelimitedReader) nativeFill(window []byte, last bool, maxRows int) int32 {
	specs, block, offsets := planPtrs(r.set)
	return int32(C.ht_fill(
		(*C.ht_state)(unsafe.Pointer(&r.core.state)),
		bytesPtr(window), C.size_t(len(window)), C.uint32_t(flag(last)),
		specs, block, offsets, C.size_t(len(r.set.specs)), C.size_t(maxRows),
		spansPtr(r.cells), C.size_t(len(r.cells)),
		bytesPtr(r.arena), C.size_t(len(r.arena)),
		(*C.ht_filled)(unsafe.Pointer(&r.core.filled))))
}

// nativeUnescape unescapes one quoted cell — a cell-table entry with its flag set names
// one — into out, and returns the bytes written.
func nativeUnescape(cell, out []byte) int {
	return int(C.hypertabular_delimited_unescape(bytesPtr(cell), C.size_t(len(cell)), bytesPtr(out), C.size_t(len(out))))
}

// planPtrs is a plan's buffers for C: nil for a plan of no columns, or none at all.
func planPtrs(set *columns) (specs *C.ht_spec, block *C.uint8_t, offsets *C.size_t) {
	if set == nil || len(set.specs) == 0 {
		return nil, nil, nil
	}
	return (*C.ht_spec)(unsafe.Pointer(unsafe.SliceData(set.specs))),
		(*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(set.block))),
		(*C.size_t)(unsafe.Pointer(unsafe.SliceData(set.offsets)))
}

func wordsPtr(words []uint64) *C.uint64_t {
	if len(words) == 0 {
		return nil
	}
	return (*C.uint64_t)(unsafe.Pointer(unsafe.SliceData(words)))
}

func slotsPtr(slots []rawSlot) *C.ht_slot {
	if len(slots) == 0 {
		return nil
	}
	return (*C.ht_slot)(unsafe.Pointer(unsafe.SliceData(slots)))
}

// nativeWorkbookStateSize is the bytes of a workbook state block.
func nativeWorkbookStateSize() int {
	return int(C.hypertabular_workbook_state_size())
}

// nativeBook makes one workbook call over scratch — with the workbook's tables when tables is
// not nil, and the plan when set is not nil — and reports into out: a *rawOpened for
// bookOpen, a *rawFilled for the rest.
func nativeBook(call bookCall, state []uint64, container []byte, s *scratch, tables *Workbook, set *columns, out unsafe.Pointer) int32 {
	var strings []byte
	var table []rawSpan
	var kinds []byte
	if tables != nil {
		strings, table, kinds = tables.strings, tables.table, tables.kinds
	}
	specs, block, offsets := planPtrs(set)
	var count, maxRows int
	if set != nil {
		count, maxRows = len(set.specs), set.batchRows
	}
	return int32(C.ht_book(C.int(call),
		wordsPtr(state), bytesPtr(container), C.size_t(len(container)),
		bytesPtr(s.window), C.size_t(len(s.window)),
		bytesPtr(s.arena), C.size_t(len(s.arena)),
		spansPtr(s.cells), C.size_t(len(s.cells)),
		slotsPtr(s.row), C.size_t(len(s.row)),
		bytesPtr(strings), C.size_t(len(strings)),
		spansPtr(table), C.size_t(len(table)),
		bytesPtr(kinds), C.size_t(len(kinds)),
		specs, block, offsets, C.size_t(count), C.size_t(maxRows),
		out))
}

// nativeSheet positions a workbook state on one sheet.
func nativeSheet(state []uint64, container []byte, info *SheetInfo, options SheetOptions, out *rawFilled) int32 {
	return int32(C.hypertabular_workbook_sheet(
		wordsPtr(state), bytesPtr(container), C.size_t(len(container)),
		bytesPtr(info.part), C.size_t(len(info.part)), C.uint32_t(info.index),
		C.uint32_t(flag(options.HasHeader)), C.uint32_t(flag(options.SkipEmptyRows)),
		(*C.ht_filled)(unsafe.Pointer(out))))
}
