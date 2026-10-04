//go:build !(cgo && !tinygo && (darwin || linux || windows) && (amd64 || arm64))

// Stand-ins for backend_static.go's functions on every build that has no backend, doing
// nothing, so that the one error the compiler reports is the identifier unsupported.go or
// unsupported_tinygo.go names, instead of one of five for the reader's missing calls.

package hypertabular

func packedVersion() uint32 { return 0 }

func (r *DelimitedReader) nativeInit(Dialect) int32                   { return errContract }
func (r *DelimitedReader) nativeHeader([]byte, bool, []rawSpan) int32 { return errContract }
func (r *DelimitedReader) nativeFill([]byte, bool, int) int32         { return errContract }
func nativeUnescape([]byte, []byte) int                               { return 0 }
