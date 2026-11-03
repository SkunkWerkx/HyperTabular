//go:build !tinygo

package hypertabular

import (
	"runtime"
	"runtime/metrics"
)

// heapBound is the heap boundHeap lets the corpus replays leave behind them.
const heapBound = 512 << 20

var heapObjects = []metrics.Sample{{Name: "/memory/classes/heap/objects:bytes"}}

// boundHeap collects once the heap passes heapBound. Stock Go's wasm heap lives in one 4 GiB
// linear memory, and nothing preempts the running goroutine there, so a GC cycle marks only
// as the suite allocates; when the GC CPU limiter, which goes by the clock, turns those
// assists off, the cycle never ends while a replay runs, and every replay's instance of
// the core is counted live. On a slow runner that reached 4 GiB mid-corpus and the runtime
// died rather than collect. A blocking collection between replays finishes the cycle,
// whatever the pacing.
func boundHeap() {
	metrics.Read(heapObjects)
	if heapObjects[0].Value.Uint64() > heapBound {
		runtime.GC()
	}
}
