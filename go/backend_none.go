//go:build !(cgo && !tinygo && !android && (darwin || linux || windows) && (amd64 || arm64) && !(ios && amd64 && !maccatalyst)) && !tinygo.wasm

// Stand-ins for the backend's functions on every build that has none (backend_static.go
// under cgo, backend_tinygo.go under TinyGo for WebAssembly), doing nothing, so that the one
// error the compiler reports is the identifier unsupported.go or unsupported_tinygo.go names,
// instead of one of five for the reader's missing calls.

package hypertabular

import "unsafe"

func packedVersion() uint32 { return 0 }

func (r *DelimitedReader) nativeInit(Dialect) int32                   { return errContract }
func (r *DelimitedReader) nativeHeader([]byte, bool, []rawSpan) int32 { return errContract }
func (r *DelimitedReader) nativeFill([]byte, bool, int) int32         { return errContract }
func nativeUnescape([]byte, []byte) int                               { return 0 }

func nativeWorkbookStateSize() int { return 0 }

func nativeBook(bookCall, []uint64, []byte, *scratch, *Workbook, *columns, unsafe.Pointer) int32 {
	return errContract
}

func nativeSheet([]uint64, []byte, *SheetInfo, SheetOptions, *rawFilled) int32 { return errContract }
