//go:build tinygo

package hypertabular

// wideSuggestionColumns is narrower under TinyGo, whose collector took 225 s over 268,000 columns, and
// 451 s once other tests had grown its heap, against a 10-minute test timeout. 1,001 rows of this many
// still ask for 400 MB, well past a sampler's 64 MiB, so the read still crosses batches; only the
// refusal past the 4 GiB limit, which the other builds reach, goes untested here.
const wideSuggestionColumns = 20_000
