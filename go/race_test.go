//go:build race

package hypertabular

// raceDetector says the suite runs under the race detector, which instruments the translated
// core too and multiplies a single-goroutine replay's time some twenty-fold.
const raceDetector = true
