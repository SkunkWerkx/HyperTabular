# Performance candidates

The candidates for the performance pass after the CRAN 1.1 gate, gathered on 2026-11-09 from where each
was found. Every number here is a receipt from the source named beside it, measured on the machine and
date given there. Candidates keep their numbers; 1, 2 and 6 landed on 2026-11-09 and are gone from here
("Landed", below). Each fix comes with its own
proof, and with the tables that print its loss re-measured. When a candidate lands, move its outcome to
the doc that holds the table (`docs/delimited.md`, `docs/workbook.md`) and the CHANGELOG, then delete it
here.

Most numbers come from the Task 23c report: `.superpowers/sdd/2026-11-05-cran-1-1-gate/task-23c-report.md`.
It was measured on an Apple M6, rustc 1.99.1, on 2026-11-08, against tag `v1.0.0` over HyperCast 1.0.0.
Its scratch builds (`~/.cache/hyper-gate/t23c/`) were deleted in the 2026-11-09 cleanup. The report still
records how to rebuild each one.

## The 2026-11-09 spike

Candidates 1, 2, 3, 6 and 10 were probed on throwaway patches: Apple M6, rustc 1.99.1, tag `v1.0.0`
over HyperCast 1.0.0 against `42efa83` over HyperCast `f558c7e`, five or six interleaved rounds with
the order rotated, median of Criterion medians (1 s warm-up, 3 s measured), load 1.4 to 2.5,
per-build spread under 2.5%. Rounds run while another program held a core were dropped. What was
learned about 3 and 10 is under each.

## Landed

- **1, the row sink's bookkeeping.** A read that asks for no header row to pass over, no widths and
  no pad or cut policy runs through a sink without those steps, on the SIMD engines only: the
  portable engine measured 2.7 to 4.4% slower through it and keeps the full sink. 2 to 3 points
  back on NEON `structure` and `structure_all_quoted`. The x86-64 engines take the lean sink and
  haven't been measured with it. Receipts: `docs/delimited.md`, "Numbers".
- **2, one bad cell per batch.** +131% is +20% against 1.0. A refused region's rows holding a bad
  byte are found from the door's offset and set aside, up to sixteen a batch; past that the rest
  is asked cell by cell, so a bad byte in every row is still +131%, and that file is now a
  benchmark scope of its own.
- **6, xlsx shared strings validated once.** `text_heavy` +17.8% is +4.1% against 1.0. The answer
  is the top bit of each string's table entry, so the ABI's layout didn't move. Receipts:
  `docs/workbook.md`, "What this release costs the core".

## The delimited core

### 3. Validation cost on text cells (cells +15%, utf8 cells +33%)

- **Measured:**
  - (2026-11-08 figures, before candidates 1, 2 and 6 landed.) `cells` +12.3% is HyperTabular's own share: validation (the region check and simdutf8), notes, and
    the options' checks.
  - Multi-byte `cells utf8` is +31.9%. It was +150% before simdutf8.
  - `batch typed` +6.6%.
- **Split (spike, 2026-11-09):** with the region check switched off, `cells` goes from +11.7% to
  +10.7% and `cells utf8` from +32.9% to +9.2%. Validation is about one point on ASCII text and about
  24 on dense multi-byte text, which is simdutf8 reading every byte and has no cheaper form here.
  A sampled profile puts the other ten points of `cells` at about 4.4 in the scan and sink, 4.5 in the
  cast loop and 0.6 in a `memset`; the cast loop's source is the same as 1.0's.
- **Ruled out by measurement:** an all-ASCII flag from the scanner (at most the one point); the text
  door's loop split by hand into a valid and an asked form (no change); the scanner's quote path made
  branchless (2 points better on fully quoted data, 2–3 worse on ordinary data); candidate 1's fix
  (nothing on `cells`).
- **New with candidate 2's fix (2026-11-09):** setting bad rows aside costs a clean read 1 to 2%:
  `cells` 8.80 → 8.96 ms and `batch` 14.14 → 14.28 ms, master against the fix alone, five interleaved
  rounds, at the edge of the spread but the same direction in three sessions. The spike's version of
  the fix measured no such cost. Not split: the per-fill `Vouched` bookkeeping, or the build's layout.
- **Not found:** where the cast loop's 4.5 points come from. The next step is the two builds'
  assembly for `column` under the text door, side by side.

### 4. A long record from a short-read stream is quadratic

- **Measured:** a 256 MiB record from a stream handing out 64 KiB a read takes 23.8 s. Each doubling of
  the record costs four times the time. A stream that fills the room it's given takes 0.064 s. Java
  (21.8 s) and Swift show the same shape. Source: `docs/delimited.md`, "Parked".
- **Cause:** an unfinished record is scanned again from its start on every fill.
- **Fix:** remember how far the scan of an unfinished record got, and resume from there. This changes
  the core's state across calls.
- **Proof:** the corpus in every binding, including every binding's stream path, plus no-panic.

### 5. Parked from the first delimited pass (`docs/delimited.md`, "Parked")

- Sep's tiered mask fast paths (blocks of pure separators, blocks of separators and newlines).
- Parallel chunk scanning: each chunk would have to start at a known row boundary, or check its guess
  against the walker's rules.

## The workbook core

### 7. Sheet sampling binds twice the columns (suggest +25–40%)

- **Measured:** a sheet's suggest reads its sample through a text door and a UUID door per column. The
  second door is about 0.45 ms per 1,000 rows, about a fifth of a whole suggest. It costs +38% on xlsx,
  +25% on ods and +39% on numbers. Source: 23c carry 1, printed in `docs/workbook.md`.
- **Ideas:** a cheaper typed signal, either a dedicated door or a kernel-side "typed" byte. That is the
  alternative Task 13c's ruling weighed.

### 8. Numbers' open (most of a `.numbers` read)

- **Measured:** 470 ms of the table's 683 ms. Sampled on 2026-10-24 (`docs/workbook.md`, "Parked"):
  - about a third is `iwa::ensure` decoding the chunk in hand from its start each time more of it is
    needed. That is quadratic in the objects a chunk holds, and the calculation-engine part holds
    thousands;
  - a sixth is the walk over the string list's entries;
  - the rest is the heap sort and the zip directory.
- **Fix:** keep the decode position in `ensure` (`rust/src/kernel/workbook/iwa.rs`) instead of restarting from the chunk's start. This
  is the same shape of fix as candidate 4.

### 9. Left over from the 2026-10-05 tokenizer pass (`docs/workbook.md`, "Parked")

- Inflate, about a sixth of a read. It must stay under the no-panic proof.
- The bounds checks in `Attrs::next`, `trimmed` and `slice`.
- ODS open's full-part scan.
- Lazy shared strings (Sylvan's forward cursor), if a huge table with a small read ever makes the
  preload the wrong trade.

## HyperCast (its own repo, its own receipts)

### 10. HyperCast 1.1 refuses bad numerals slower (`faulty` +14.5%)

- **Measured:** under 1.0's kernel, HyperCast 1.1 refuses `123x`, `y123` and `123z` 14.5% slower than
  1.0 did. The cause inside HyperCast hasn't been split.
- **Isolated (spike, 2026-11-09):** both versions linked into one program, 100,000 cells a pass, 61
  alternated passes, run twice. `cast_i32` on `123x` 19.4 → 23.0 ns (+17–19%), on `y123` 11.3 → 13.1 ns
  (+17–18%), `cast_f64` on `123z` 20.6 → 23.9 ns (+16%). Numerals that read are level or faster
  (`cast_i32` −9%, `cast_f64` +1%).
- **Not the cause:** building the `Failure` from its site, which is a few moves.
- **First step:** a receipt in HyperCast's `docs/benchmarks.md`: a `*-malformed` row against tag
  `v1.0.0`. Its benchmarks only compare numerals that read.
- **Then:** profile the engine's failure lane; that is where the 3.5 ns are.

## The bindings

### 11. Ruby's Fiddle path reads workbooks slower than the extension

- **Measured:** Fiddle reads an xlsx sheet 54% slower and an ods sheet 30% slower than the Magnus
  extension (`ruby/README.md`, 2026-11-08). A three-column sheet read level on 2026-11-07, before the
  2026-11-08 changes to Ruby's read path. The cause isn't known.
- **First step:** profile the Fiddle sheet read before and after the 2026-11-08 changes.

### 12. Go's translated core (1.7–2.3x slower than cgo)

- **Status:** the loss stays printed (`go/README.md`). cgo comes back as an opt-in backend, probably at
  2.0, with the translated core staying the default until then.
- **In this pass:** only the translation's own costs, if anything shows up in a profile. Any core
  candidate above that lands also reaches Go through `go generate`.

## How to measure

Every claim is measured, and losses stay printed next to wins (CLAUDE.md, "Claims are measured").

- **Builds:**
  - one build is tag `v1.0.0` over HyperCast 1.0.0;
  - when HyperCast's share matters, a third build of 1.0's `rust/` over HyperCast 1.1 splits each loss
    between the two libraries;
  - every build runs in a scratch copy, so the repo's lockfiles never move.
- **Runs:**
  - five or six rounds, interleaved, with the build order rotated each round;
  - report the median of Criterion medians;
  - note the load average;
  - run under `heavy.lock`, one heavy job at a time.
- **Noise:** the build is layout-sensitive, and this machine's noise is 3–5%. An A/B needs two saved
  binaries run alternately, and a result inside the noise counts as no change.
- **Tables:**
  - a new table replaces the old one only when every row was re-run on one machine in one session;
  - never mix machines;
  - a row that predates a change says so.
- **Proofs for a core change:**
  - `cargo test`;
  - `rust/check-core.sh` in Docker (Linux only), with its no-panic canary;
  - the corpus in every binding;
  - `HYPERTABULAR_BLESS=1 cargo test --test header` if an export's types moved;
  - `go generate` and the regeneration check.

## Not core performance, parked here so it isn't lost

- **macOS runners per board:** the board asks for 16 macOS runners (13 `xcode-27`, 3 `macos-latest`), out
  of about 100 jobs. When GitHub is short of Macs, one of them gets dropped. On 2026-11-09 the osx-arm64
  wheel build was dropped twice with "not acquired by Runner of type hosted". One macOS job that builds
  the core once and runs every binding's suite in turn would cut that to about 6. That's a Forge change
  reaching HyperCast and HyperUuid too.
- **`r/configure` hard-codes rustc 1.89** as the floor of the crates it builds. When a floor moves, it
  has to be moved by hand with the rest. It could be derived from `rust-version` instead.
