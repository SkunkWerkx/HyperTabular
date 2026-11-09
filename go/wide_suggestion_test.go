//go:build !tinygo

package hypertabular

// wideSuggestionColumns is a source no single batch of a default sample fits: 1,001 rows of it ask for
// 5.4 GB, past the limit.
const wideSuggestionColumns = 268_000
