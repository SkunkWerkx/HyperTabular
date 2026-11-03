//go:build !wasm || tinygo

package hypertabular

// boundHeap is a no-op off stock Go's wasm (heap_wasm_test.go).
func boundHeap() {}
