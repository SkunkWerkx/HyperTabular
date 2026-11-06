# Go `-unsafe` Core Translation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Translate the HyperTabular, HyperCast and HyperUuid cores with wasm2go's `-unsafe`, prove the bounds checks and the big-endian path still hold, and keep that proof running in the Forge.

**Architecture:** The flag lives in each repo's `go/internal/wasm2go/main.go`, and each `core.go` is regenerated from it. A new test in `go/internal/core` pins the generated helpers. One Forge script runs each Go suite on s390x under QEMU, for both the local gate and a new `hyper-test-go.yml` job.

**Tech Stack:** Go 1.27.3 (`go` line 1.26), wasm2go v0.4.20, rustc 1.99.1, Docker with QEMU, GitHub Actions, Python `unittest` for the Forge contracts, benchstat.

**Spec:** `docs/superpowers/specs/2026-11-05-go-unsafe-core-design.md`

## Global Constraints

- The owner drives git. No commit, push, tag, dispatch, Forge merge or companion pin move appears in this plan. Each task ends with its changes verified in the working tree.
- A verification counts only when its negative control fails.
- No probe number goes into a table. Tables take only the measured runs in Task 6.
- `core.go` is only ever written by `go generate` in `go/`.
- HyperTabular's Go and Cargo steps need local overrides until HyperCast 1.1.0 is on a registry. Keep them out of the tree: a `go.work` with a `replace`, and a `CARGO_HOME` `[patch]`, restoring `rust/Cargo.lock`'s hypercast entry afterwards.
- CRAN stays held: nothing here changes either R package's sources.

## Review Focus

- An access straddling the last byte of memory should panic rather than read past the slice. Pinned by `TestAccessPastTheEndPanics`.
- An address near 2^32 should panic rather than wrap to a small valid address. Pinned by `TestAddressNearFourGiBPanics`.
- A value at any byte offset, aligned or not, should read back the value written. Pinned by `TestEveryOffsetRoundTrips`.
- Memory that grows should be reachable at once at its new addresses, with no stale pointer. Pinned by `TestAccessAfterGrowth`.
- A CPU that takes the `encoding/binary` path should keep the same guarantees. Pinned by running the bounds test on riscv64 in Task 3.

---

### Task 1: Dependency sweep

**Files:**
- Modify: each repo's manifests, lockfiles and toolchain pins, as `git status` reports them

**Interfaces:**
- Produces: the rustc and Go versions every later task measures on, recorded in the report

- [ ] **Step 1: Run `git fetch` in all three repos and the Forge, and compare with origin.** Expected: no repo behind. If one is, stop and report.
- [ ] **Step 2: Move every dependency and toolchain pin to latest in each repo, using the list in `CLAUDE.md`, "Upstream: HyperCast", and Dependabot's ecosystems.** wasm2go stays at v0.4.20 unless a newer one has appeared. HyperCast's own version stays out of it: that bump happens only once 1.1.0 is published.
- [ ] **Step 3: Regenerate each repo's core if anything reaching its wasm moved: rustc, wasm2go or a Rust dependency of the core.** If rustc moved, change `rust-toolchain.toml` in all three repos together, keeping wasm2go and the rustc pin equal across them. Then run `go generate` in each affected `go/`. Step 4's regeneration check catches a core this step missed.
- [ ] **Step 4: Run each repo's Go suite and regeneration check.** `CGO_ENABLED=0 go test -count=1 ./...`, then `<PREFIX>_CHECK_CORE=1 go test -count=1 -run 'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .` in `go/`, with `<PREFIX>` being `HYPERTABULAR`, `HYPERCAST` or `HYPERUUID`. The variable is required, because without it a missing prerequisite skips the check. Expected: PASS and no skips. For HyperTabular, use the overrides.
- [ ] **Step 5: Verify every other ecosystem the sweep changed, with the commands in that repo's project notes and CI.** For each repo, list what `git status` shows changed, by ecosystem, and run that binding's or tree's tests and linters, including the Forge's own script tests if its pins moved. An ecosystem with no changes needs no run, and the report says which those were. Whatever only CI can prove is named in the report. A sweep change is reported as verified only if its check ran here.
- [ ] **Step 6: Name every package a real constraint held back, and why.**

### Task 2: The Forge's Go s390x job

**Files:**
- Create: `HyperForge/.github/scripts/go_big_endian.sh`
- Modify: `HyperForge/.github/workflows/hyper-test-go.yml`, `HyperForge/.github/scripts/companion_map.py:182-183`
- Test: `HyperForge/.github/scripts/test_hyper_test_bindings.py`, `test_companion_map.py`

**Interfaces:**
- Produces: `go_big_endian.sh <go-dir>`, with environment `ARCH` (default `s390x`), `IMAGE` (default `golang:<go.mod minor>`) and `PACKAGES` (default every package in `go list ./...`). `GOFLAGS` and `GOWORK` reach the cross-build, so overlays and workspaces work. Exit status is non-zero if any package's suite fails.

- [ ] **Step 1: Write the failing contract tests in `test_hyper_test_bindings.py`.**

```python
def test_go_runs_on_a_big_endian_host_off_a_pull_request(self):
    _, workflow = load('go')
    job = workflow['jobs']['test-big-endian']
    self.assertIn("inputs.mode != 'pr'", job['if'])
    self.assertEqual(job['runs-on'], 'ubuntu-26.04')
    uses = [s.get('uses', '') for s in job['steps']]
    self.assertTrue(any(u.startswith('docker/setup-qemu-action@') for u in uses))
    self.assertIn('./.hyperforge/.github/actions/fetch-companions', uses)
    suite = next(s for s in job['steps'] if s.get('name') == 'Go on s390x, under QEMU')
    self.assertIn('go_big_endian.sh', suite['run'])
```

Add `'hyper-test-go': 'test-big-endian'` as a second entry beside `test_musl_is_off_on_a_pull_request`'s, in a test of its own. Add a `test_companion_map.py` case that `hyper-test-go.yml:test-big-endian:linux-x64` resolves to the `go` artifacts.

- [ ] **Step 2: Run them.** `python3 -m unittest test_hyper_test_bindings test_companion_map` in `.github/scripts`. Expected: FAIL, `KeyError: 'test-big-endian'`.
- [ ] **Step 3: Write `go_big_endian.sh`.**

For each package, cross-build with `CGO_ENABLED=0 GOOS=linux GOARCH=$ARCH go test -c -o <tmp>/<n>.test <pkg>`, skipping a package with no test files. Run each binary with `docker run --rm --name <unique> --platform linux/$ARCH -v <repo>:<repo> -v <tmp>:/bin-under-test:ro -w <package dir> $IMAGE`, passing `-test.count=1 -test.timeout=30m`. Inside the container, first check that `uname -m` is the machine `ARCH` names (`s390x`, `riscv64`, or `mips64` for `mips64le`), and fail if not. Tests that skip themselves in that Go-only container stay skipped, and the script's output records them: the regeneration tests, which need cargo, and HyperCast's `downgrade_test.go`, which skips unless `HYPERCAST_DOWNGRADE_ROOT` is set and then also needs the regeneration prerequisites. Both run on the regular host legs instead. A `trap` must `docker rm -f` the running container on exit, so an interrupted run leaves nothing behind. `set -euo pipefail`, and POSIX-safe quoting in the style of the neighbouring scripts.

- [ ] **Step 4: Add the `test-big-endian` job to `hyper-test-go.yml`.**

`if: inputs.mode != 'pr'`, on `ubuntu-26.04`. Its steps are the `test-musl` job's checkout, revision validation, Forge checkout, `setup-go` and `fetch-companions` (`leg: hyper-test-go.yml:test-big-endian:linux-x64`). Then `docker/setup-qemu-action@99012661954931238ded8c8b007157a8430204e1 # v4.4.0` with `platforms: s390x` (the pin in HyperTabular's `ci.yml`, or newer if Task 1 moved it), then a step named `Go on s390x, under QEMU` that runs `.hyperforge/.github/scripts/go_big_endian.sh go`. Update the workflow's header comment and `GoWorkflowTests`' docstring, which count the hosts.

- [ ] **Step 5: Add `('hyper-test-go.yml', 'test-big-endian'): ('go',),` to `companion_map.py`.**
- [ ] **Step 6: Run the Forge's script tests.** `python3 -m unittest discover -p 'test_*.py'` in `.github/scripts`. Expected: PASS, including any test of `.sh` shape or of workflows' pinned actions.
- [ ] **Step 7: Run the script on all three repos with today's safe translation.** Expected: PASS on each. HyperTabular took about 6.5 minutes. If a suite fails here, stop and report: the failure predates the flag.
- [ ] **Step 8: Run the negative control.** Run the script on HyperTabular with `GOFLAGS=-overlay=<json>` pointing `core.go` at a scratch `-unsafe` translation with its helpers' `if big { ... }` byte swaps removed. Expected: non-zero exit with suite failures. Report the output.

### Task 3: Flag, regenerated cores and bounds test

**Files:**
- Create: `go/internal/core/bounds_test.go` in each of HyperCast, HyperTabular and HyperUuid
- Modify: `go/internal/wasm2go/main.go` (HyperCast:112, HyperTabular:109, HyperUuid:106), `go/internal/core/core.go`

**Interfaces:**
- Consumes: the generated `load16/32/64[T uint32|uint64](mem []byte, addr T)`, `store16/32/64[T uint32|uint64](mem []byte, addr T, val)`, `memory_grow(mem *[]byte, delta, max int64) int64`, and `New()` (HyperUuid: `New(nil)`)
- Produces: `TestAccessPastTheEndPanics`, `TestAddressNearFourGiBPanics`, `TestEveryOffsetRoundTrips`, `TestAccessAfterGrowth`, `TestAnExportPastTheEndPanics`, all in package `core`

Work through HyperCast, then HyperTabular, then HyperUuid. The test is the same in each except for the export it calls.

- [ ] **Step 1: Write `bounds_test.go`, tagged `//go:build !tinygo`.** Use a 64-byte buffer and a table over all six helpers, with widths 2, 4 and 8.

```go
// TestAccessPastTheEndPanics, for each width w and both index types:
//   addr := len(mem)-w        → no panic
//   addr := len(mem)-w+1      → panic; for a store, mem is byte-for-byte unchanged
// TestAddressNearFourGiBPanics: addr uint32(0xFFFFFFFF) and uint64(1<<32), for all six → panic
// TestEveryOffsetRoundTrips: for offset 0..8 and each width, store 0x0102030405060708
//   truncated to the width; the load returns it; and mem[offset:offset+w] equals
//   binary.LittleEndian.AppendUintN of it, on every host.
// TestAccessAfterGrowth: 64 KiB mem; load32 at len(mem) panics; memory_grow(&mem, 1, 4);
//   a store then load at the old len(mem) returns the value.
// TestAnExportPastTheEndPanics: const past = int32(0x7fffffff)
//   HyperTabular: New().Xhypertabular_delimited_init(past, past) panics
//   HyperCast:    New().Xcast_bool(past, past, 16, past) panics
//   HyperUuid:    New(nil).Xuuid_get_timestamp(past, past, past) panics
```

If an export validates its arguments and returns a code instead of panicking, choose another pointer-taking export. Say so in a comment on the test.

- [ ] **Step 2: Run it against the safe translation.** `go test ./internal/core`. Expected: PASS, which shows the test describes today's guarantee.
- [ ] **Step 3: Add `"-unsafe"` to the `go tool wasm2go` arguments in `main.go`.** Add a comment: the flag replaces the helpers with `unsafe.Add` access behind a check of the access's last byte, wasm2go keeps the `encoding/binary` path for CPUs that do not allow unaligned access, and `bounds_test.go` pins the checks.
- [ ] **Step 4: Run `go generate` in `go/`, then `go test ./internal/core`.** Expected: PASS.
- [ ] **Step 5: Run the bounds negative control.** In a scratch copy (an overlay, never the tree), delete the `_ = mem[...]` lines from the helpers. Expected: `TestAccessPastTheEndPanics` and `TestAddressNearFourGiBPanics` FAIL. An out-of-process crash also counts. Report the output.
- [ ] **Step 6: Run the regeneration negative control.** With `core.go` translated without the flag through a scratch overlay of the generator, or by removing the flag temporarily and restoring it with `git diff` proving it restored: `<PREFIX>_CHECK_CORE=1 go test -count=1 -run 'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .`. Expected: FAIL. `<PREFIX>` is `HYPERTABULAR`, `HYPERCAST` or `HYPERUUID`.
- [ ] **Step 7: Run the bounds test on the fallback path.** `ARCH=riscv64 PACKAGES=./internal/core go_big_endian.sh go`, after confirming `unalignedOK` is false for riscv64 in the generated helper. If riscv64 is true, use `mips64le`. Expected: PASS.

### Task 4: Verification across the three repos

**Files:**
- Test only; a failure goes back to Task 3

- [ ] **Step 1: Run in each `go/`.**

```sh
CGO_ENABLED=0 go test -count=1 ./...
CGO_ENABLED=1 go test -race -count=1 ./...
go vet ./... && gofmt -l . && revive -config revive.toml ./...
<PREFIX>_CHECK_CORE=1 go test -count=1 -run 'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .
GOOS=js GOARCH=wasm PATH="$(go env GOROOT)/lib/wasm:$PATH" go test -count=1 ./...
GOOS=wasip1 GOARCH=wasm PATH="$(go env GOROOT)/lib/wasm:$PATH" go test -count=1 ./...
```

Expected: all PASS, and `gofmt -l` prints nothing beyond what the repo's own lint excludes. Copy the flags from `hyper-test-go.yml` and `hyper-build-wasm.yml` if they differ.

- [ ] **Step 2: Run TinyGo in each repo with the corpus mounted, the way `hyper-build-wasm.yml:625` builds it, plus `go/internal/tinygosmoke`.** Expected: PASS, with `bounds_test.go` excluded by its tag.
- [ ] **Step 3: Run the s390x gate on each repo's regenerated core.** `.github/scripts/go_big_endian.sh go` from the Forge. Expected: PASS on all three. This gate is required: if it can't run, the work is blocked.
- [ ] **Step 4: Report each result by repo, and name anything that only CI can prove.** The new Forge job itself can't run until it's merged.

### Task 5: Controlled safe-versus-unsafe comparison

**Files:**
- No repo files. Results stay in the scratchpad.

**Interfaces:**
- Produces: a benchstat table per repo, and a go or stop decision

- [ ] **Step 1: In each repo, translate one wasm twice:** `go tool wasm2go -pkg core` and `go tool wasm2go -unsafe -pkg core`, into the scratchpad, with overlay JSON for each. Confirm the plain one equals `git show HEAD:go/internal/core/core.go`, or Task 1's regeneration of it.
- [ ] **Step 2: Run the two interleaved in one session on the quiet M6.** Use six runs each, with the iteration counts each repo's table states (HyperTabular: `-bench 'Delimited|Workbook' -benchmem`, three iterations a run for workbooks).
- [ ] **Step 3: Run `benchstat safe.txt unsafe.txt`.** If any row is a significant loss, stop and report. Don't ship the flag on some repos only. Otherwise continue.

### Task 6: Published tables and docs

**Files:**
- Modify: `go/README.md` in all three (HyperTabular:254, 386-401; HyperCast:76, 769; HyperUuid:120, 604), `docs/benchmarks.md` in HyperCast and HyperUuid, `CHANGELOG.md` under Unreleased in all three, `go/internal/wasm2go/main.go` comments where they describe the flags

**Interfaces:**
- Consumes: the regenerated cores from Task 3

- [ ] **Step 1: Re-run each published Go table the way its caption says.** For HyperTabular that's six interleaved runs per binding, with the cgo binding at `93e0807`, in one session on the M6.
- [ ] **Step 2: Replace the rows, with date, machine, method and every remaining loss, and the memory figures.** HyperTabular's table says it predates the quote fix (checklist row 1). Any row not re-run keeps its label, and rows measured on another machine are not mixed in.
- [ ] **Step 3: Update `CLAUDE.md`'s "2–3.7x" sentence and any other prose quoting the old figures.** `grep -rn '3\.7x' README.md CHANGELOG.md CLAUDE.md docs go` in each repo. Expected afterwards: only dated history remains.
- [ ] **Step 4: Point the "bounds-checks every memory access" sentences at `internal/core/bounds_test.go`, and add a CHANGELOG bullet per repo:** the core is translated with `-unsafe`, with the measured gain and the remaining loss, and accesses are still bounds-checked.
- [ ] **Step 5: Run the README examples touched, and `go test ./...` once more in each repo.** Expected: PASS.
- [ ] **Step 6: Report what's ready, by repo and by Forge file, with the two owner steps:** merging the Forge job with no preparation in flight, and moving the companion pin after a green board run on the new HyperCast commit.
