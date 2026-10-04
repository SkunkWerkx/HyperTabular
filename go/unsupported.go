//go:build !tinygo && !(cgo && (darwin || linux || windows) && (amd64 || arm64))

// Every stock-Go build the backend does not cover lands here, and stops: CGO_ENABLED=0
// (Go's default for a cross-compile, and whenever no C compiler is found), GOOS=wasip1 and
// js, and any platform there is no archive for. Go has no #error, so the stop is a reference
// to an identifier that does not exist, named to read as the explanation in the compiler's
// "undefined:" message — the way HyperCast's Go module stops the same builds, and it stops
// them first: the hypercast package this one imports is compiled before this one is.
//
// TinyGo has a file of its own, unsupported_tinygo.go, because its answer is different.

package hypertabular

var _ = hypertabular_needs_cgo_and_a_C_compiler_on_linux_darwin_or_windows_amd64_arm64__set_CGO_ENABLED_1
