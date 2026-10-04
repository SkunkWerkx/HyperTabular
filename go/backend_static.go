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
// Windows on amd64 carries one flag more, -Wl,--allow-multiple-definition, and it is there
// for GNU ld alone. Both MSVC archives keep Rust's own 128-bit division helpers (MSVC has
// none), which LLVM emits for that target as COFF weak externals: __divti3, defaulting to a
// global named .weak.__divti3.default. GNU ld does not take such a default as the
// definition — the calls resolve to libgcc's __divti3, last on gcc's link line, exactly as
// they do when HyperCast's archive is linked alone — but it does keep searching later
// archives for one, pulls the same helper out of the second archive, and then refuses the
// two identical .weak.*.default globals as a multiple definition. The flag lets the first
// stand; neither is called. It applies to the whole link, which is the cost: a duplicate
// symbol elsewhere in a program that imports this module is no longer an error there. lld —
// llvm-mingw's linker, the one arm64 builds with — resolves a weak external to its default,
// never loads the second helper, and needs no flag.
//
// # What crosses, and the cgo pointer rules
//
// One call per batch: ht_fill below. Everything it is handed is Go memory, and the rules for
// passing Go pointers to C (cmd/cgo, "Passing pointers") are met by construction rather
// than by pinning:
//
//   - Every pointer argument points to memory with no Go pointer in it — bytes, spans,
//     specs, the core's state block, the block of column buffers. Such a pointer may be
//     passed as it is, and what it points to is pinned for the duration of the call.
//   - The core's ColumnBuffer array is the one shape that holds pointers: per column, where
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
#cgo linux,amd64 LDFLAGS: ${SRCDIR}/staticlib/linux_amd64/libhypertabular.a
#cgo linux,arm64 LDFLAGS: ${SRCDIR}/staticlib/linux_arm64/libhypertabular.a
#cgo darwin,amd64 LDFLAGS: ${SRCDIR}/staticlib/darwin_amd64/libhypertabular.a
#cgo darwin,arm64 LDFLAGS: ${SRCDIR}/staticlib/darwin_arm64/libhypertabular.a
#cgo windows,amd64 LDFLAGS: ${SRCDIR}/staticlib/windows_amd64/libhypertabular.a -Wl,--allow-multiple-definition
#cgo windows,arm64 LDFLAGS: ${SRCDIR}/staticlib/windows_arm64/libhypertabular.a
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

// The core's C ABI — rust/src/kernel/exports.rs, the six exports.
uint32_t hypertabular_version(void);
size_t hypertabular_delimited_state_size(void);
int32_t hypertabular_delimited_init(ht_state *state, const ht_dialect *dialect);
int32_t hypertabular_delimited_header(ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
                                      ht_span *names, size_t names_cap, uint8_t *arena, size_t arena_cap, ht_filled *out);
int32_t hypertabular_delimited_fill(ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
                                    const ht_spec *specs, const ht_buffer *columns, size_t column_count, size_t max_rows,
                                    ht_span *cells, size_t cells_cap, uint8_t *arena, size_t arena_cap, ht_filled *out);
size_t hypertabular_delimited_unescape(const uint8_t *cell, size_t len, uint8_t *out, size_t cap);

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

// One batch. Column i's values are at block + offsets[2i] and its verdicts at
// block + offsets[2i + 1]; the ColumnBuffer array the core reads them from is built here,
// in C memory that does not outlive the call — see the note at the top of this file.
static int32_t ht_fill(ht_state *state, const uint8_t *input, size_t input_len, uint32_t last,
                       const ht_spec *specs, uint8_t *block, const size_t *offsets, size_t column_count, size_t max_rows,
                       ht_span *cells, size_t cells_cap, uint8_t *arena, size_t arena_cap, ht_filled *out) {
	ht_buffer on_stack[HT_STACK_COLUMNS];
	ht_buffer *columns = on_stack;
	if (column_count > HT_STACK_COLUMNS) {
		columns = (ht_buffer *)malloc(column_count * sizeof(ht_buffer));
		if (columns == NULL) {
			return HT_ERR_SHIM_MEMORY;
		}
	}
	for (size_t i = 0; i < column_count; i++) {
		columns[i].values = block + offsets[2 * i];
		columns[i].verdicts = (ht_verdict *)(block + offsets[2 * i + 1]);
	}
	int32_t code = hypertabular_delimited_fill(state, input, input_len, last, specs, columns, column_count, max_rows,
	                                           cells, cells_cap, arena, arena_cap, out);
	if (columns != on_stack) {
		free(columns);
	}
	return code;
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
	var specs *C.ht_spec
	var block *C.uint8_t
	var offsets *C.size_t
	if len(r.specs) > 0 {
		specs = (*C.ht_spec)(unsafe.Pointer(unsafe.SliceData(r.specs)))
		block = (*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(r.block)))
		offsets = (*C.size_t)(unsafe.Pointer(unsafe.SliceData(r.offsets)))
	}
	return int32(C.ht_fill(
		(*C.ht_state)(unsafe.Pointer(&r.core.state)),
		bytesPtr(window), C.size_t(len(window)), C.uint32_t(flag(last)),
		specs, block, offsets, C.size_t(len(r.specs)), C.size_t(maxRows),
		spansPtr(r.cells), C.size_t(len(r.cells)),
		bytesPtr(r.arena), C.size_t(len(r.arena)),
		(*C.ht_filled)(unsafe.Pointer(&r.core.filled))))
}

// nativeUnescape unescapes one quoted cell — a cell-table entry with its flag set names
// one — into out, and returns the bytes written.
func nativeUnescape(cell, out []byte) int {
	return int(C.hypertabular_delimited_unescape(bytesPtr(cell), C.size_t(len(cell)), bytesPtr(out), C.size_t(len(out))))
}
