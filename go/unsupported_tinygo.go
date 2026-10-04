//go:build tinygo

// TinyGo lands here, and stops. HyperCast's Go module has a TinyGo backend for WebAssembly,
// so importing it under TinyGo builds; this module has none, and will not: the tabular
// layer is out of scope for WebAssembly, and no archive is linked for it. Building with
// stock Go and cgo is the fix, and — Go having no #error — the identifier below is how the
// compiler is made to say so.

package hypertabular

var _ = hypertabular_has_no_TinyGo_or_WebAssembly_backend__the_tabular_layer_is_native_only__build_with_stock_Go_and_cgo
