# Go: translate the cores with wasm2go's `-unsafe`

Date: 2026-11-05. Status: design approved in conversation; this written spec awaits review.

## Outcome

HyperTabular's, HyperCast's and HyperUuid's Go bindings read faster, stay cgo-free on every
GOOS and GOARCH they build for today, and keep their promise that a core bug is a Go panic and
never a corrupted heap. The losses that remain against native code stay printed.

## Constraints

- HyperCast 1.1 and HyperTabular 1.1 are held together, so this work sets no release date and
  does not need to land ahead of HyperTabular's checklist rows.
- A change reaches every sibling repo in the same pass.
- The owner drives git. Every change stays in the three working trees until the owner asks for
  a commit, a push or a pin move. This spec is not committed either.
- Benchmarks are dated, machine-specific receipts. No number here goes into a table.

## Evidence

Throwaway probes on 2026-11-05 on an Apple M6, with Go 1.27.3 and an overlay over the committed
HyperTabular `core.go`. The wasm the probes translated reproduced the committed `core.go` byte
for byte under the current flags.

| Read | Today | `-unsafe` |
| --- | ---: | ---: |
| Delimited, 22 MB, from memory | 59.5 ms | 33.9 ms |
| `excel-win-300k.xlsx` | 690 ms | 482 ms |
| `libreoffice-300k.ods` | 1311 ms | 1175 ms |
| `numbers-300k.numbers` | 1297 ms | 1110 ms |

Delimited is three runs of 40 iterations. Each workbook is one run of two iterations.

These gave no speedup: PGO over a delimited profile, `wasm-opt -O3`, and caching `m.memory` in a
local. Expanding the memory helpers inline was about 3% slower. A shorter helper body was about
9% slower.

The suite, with the regeneration tests skipped, passed against the `-unsafe` translation with
and without `-race`. `go build ./...` passed for linux/amd64, linux/s390x, linux/riscv64,
windows/arm64, js/wasm and wasip1/wasm. TinyGo was not run.

HyperTabular's suite, with the regeneration tests skipped, also passed on linux/s390x under QEMU
in Docker with both the current and the `-unsafe` translation, in about 6.5 minutes each. With the
helpers' byte swaps removed from the `-unsafe` translation, the same suite failed.

## What `-unsafe` changes

wasm2go swaps the bodies of `load16/32/64` and `store16/32/64`. On a CPU it marks as allowing
unaligned access, the helper checks the last byte of the access (`_ = mem[uint64(addr)+3]`), then
reads or writes through `unsafe.Add`. A big-endian host byte-swaps the value. Any other CPU takes
the same `encoding/binary` path as today, chosen by `unalignedOK` at compile time.

The bounds check stays, but its form changes, and the bindings' READMEs rely on it. That is why
this design adds a test.

## Changes

In each of the three repos:

1. `go/internal/wasm2go/main.go`: add `-unsafe` to the `go tool wasm2go` arguments, with a comment
   saying what the flag does and that the bounds check is pinned by the new test.
2. `go/internal/core/core.go`: regenerate with `go generate` in `go/`.
3. `go/internal/core/bounds_test.go`, new, described below.
4. `go/README.md`: update the generator paragraph and the benchmark section. The "bounds-checks
   every memory access" sentences stay, and now point at the test.
5. `docs/benchmarks.md` where the repo has one, and `CHANGELOG.md` under Unreleased.

HyperTabular's `CLAUDE.md` gains nothing: the flag lives in the generator, which regeneration
already covers. HyperCast's and HyperUuid's project notes change only if they describe the flags.

In the Forge: the s390x job, the script it shares with the local run, its entry in
`companion_map.py`, and the contract tests that hold them. Merging it reaches every consumer at
once, so it is the owner's step, and only with no preparation in flight.

HyperUuid's change goes to its master for its next release. Its R package is not touched, so
nothing moves for its CRAN submission.

## The bounds test

`go/internal/core/bounds_test.go`, in package `core`, so it reaches the generated helpers:

- For each of `load16`, `load32`, `load64`, `store16`, `store32` and `store64`, an access starting
  at `len(mem)-width+1` must panic, an access starting at `len(mem)-width` must succeed, and a
  store that panics must leave the buffer unchanged.
- Each store must write little-endian bytes and each load must read them back, whatever the host,
  so the suite asserts the guest's byte order directly.
- The same for an address far past the end, through both the `uint32` and `uint64` instantiations
  the generated code uses.
- One call through a real export with a pointer past the end of memory must panic.
- Tagged `!tinygo`: TinyGo's wasm targets cannot recover a panic (`HyperCast/go/cast_test.go` says
  so), so the panic assertions cannot run there.

**Negative control:** in a scratch copy, delete the `_ = mem[...]` lines from the helpers. The test
must fail. Report the output.

## Verification

Each repo's Go tree, against its regenerated core:

- `go test ./...` with `CGO_ENABLED=0`.
- `go test -race ./...`, which also runs `checkptr`.
- `go vet ./...` and the repo's Go linter (`revive`).
- `tinygo test -target=wasip1` with the corpus mounted, and the repo's TinyGo smoke test.
- `GOOS=js` and `GOOS=wasip1` with `GOARCH=wasm`, running `go test ./...`.
- `HYPERTABULAR_CHECK_CORE=1 go test -count=1 -run
  'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .`, and each sibling's equivalent.
  Negative control: remove the flag from the generator in a scratch copy, and the diff must fail.
- Big-endian, required before this work counts as verified: cross-build each repo's test binary
  (`GOOS=linux GOARCH=s390x go test -c`) and run the whole suite, corpus and bounds test included,
  in a `linux/s390x` container under QEMU. CI cannot stand in for this: the Forge's
  `hyper-test-go.yml` only runs `go vet` for s390x, the core's s390x job runs Rust tests rather than
  these Go helpers, and `TestDecodingForABigEndianHost` tests the binding's decoding rather than
  wasm2go's byte swap. Negative control: remove the helpers' byte swaps in a scratch copy, and the
  suite must fail. If a local run stops working, this stays blocked rather than deferred.
- A new Go s390x job in the Forge's `hyper-test-go.yml` keeps this proven on every later
  regeneration, on the board and in a release, and off on a pull request like the musl legs. It
  runs the same script as the local gate. The local run still gates this work, because the job
  cannot run until the Forge change is merged.

HyperTabular's regeneration tests resolve HyperCast 1.1.0, which is not on a registry yet, so they
need the local overrides `CLAUDE.md` describes, kept out of the tree.

## Numbers

- Decide on the flag first, with a controlled comparison in each repo after the dependency sweep:
  translate one wasm file twice, with and without `-unsafe`, and run the two interleaved in one
  session with the same counts as that repo's table, compared with benchstat. Only the flag differs
  between them, so a difference is the flag's and not the toolchain's.
- Then re-run each repo's published table the way it was measured, as a separate comparison. For
  HyperTabular that is six interleaved runs per binding, three iterations a run for workbooks,
  benchstat medians, and the cgo binding at `93e0807` run in the same session.
- Print every remaining loss, and the memory figures. State that HyperTabular's table predates
  the quote fix (checklist row 1).
- If `-unsafe` shows a significant loss against the safe translation on any row in any repo, stop
  and report it rather than shipping or dropping the flag on that repo alone. The cgo comparison
  never triggers this stop, because it also spans the toolchain and core changes since `93e0807`.

## Order

1. Take each repo's dependencies and toolchain pins to latest, as a separate pass, naming whatever
   a constraint holds back. wasm2go is already at its latest, v0.4.20. Moving rustc means
   regenerating, so this comes before any measurement.
2. Generator flag, regenerated core and bounds test, HyperCast first and then the other two.
3. Verification, repo by repo.
4. The safe-versus-unsafe comparison, then the published tables and docs.
5. Report what is ready. Moving the companion pin, which needs a green
   board run on the new HyperCast commit, is the owner's step.

## Out of scope

- Changing the Rust core for Go's sake.
- Moving off wasm2go, or hand-porting anything to Go.
- The optional cgo tag and the assembly scanner. Both are decided after checklist row 1 lands,
  because that fix changes how the scanner tracks quotes and may change the native numbers too.
  Row 1's own work carries a benchmark row, so any native cost it adds gets printed.
- The CRAN checklist corrections, which are a separate pass.
