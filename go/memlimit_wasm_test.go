//go:build !tinygo

package hypertabular

import "runtime/debug"

// Stock Go's wasm heap lives in one 4 GiB linear memory, and the runtime dies on an
// allocation that would grow it past that rather than collect first. The GC's goal is a
// multiple of the live heap, and on a slow runner the live heap a cycle measures (what the
// corpus replays allocated during a long mark) is enough for that goal to pass 4 GiB, so
// the suite died mid-corpus. A soft limit at half the address space makes the GC collect
// before then, with room left for a guest memory that grows by copying.
func init() {
	debug.SetMemoryLimit(2 << 30)
}
