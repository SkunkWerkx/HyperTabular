//go:build tinygo && !tinygo.wasm

// TinyGo lands here, and stops, on every target but WebAssembly: backend_tinygo.go links
// the core for the browser and for WASI, and there is no archive for TinyGo's
// microcontroller and bare-metal targets, which have no room for a workbook anyway. Building
// with stock Go and cgo is the fix off WebAssembly, and — Go having no #error — the
// identifier below is how the compiler is made to say so.

package hypertabular

var _ = hypertabular_has_TinyGo_backends_only_for_WebAssembly__build_with_stock_Go_and_cgo_or_tinygo_target_wasm_or_wasip1
