# HyperTabular adopts HyperCast's checked doors — design

Date: 2026-11-11. Status: approved in conversation, section by section; this document awaits the
owner's review before an implementation plan is written.

## Goal

HyperTabular declares HyperCast's OpenAPI 3.2 constraints per column and reports HyperCast's
checked verdict per cell, before HyperCast 1.1 is tagged, so that integration finds any HyperCast
fix that has to be backported. When the pass lands, HyperCast 1.1 needs only versions set
everywhere, the CHANGELOG and documents current, and shipping.

Success:

- Every HyperTabular binding (11) declares constraints on a column and reads the checked verdict,
  with parity in behaviour, API and wording.
- A shared corpus proves each cell's verdict equals HyperCast's for the same input.
- Every proof in both repos holds with the checked paths linked.
- HyperTabular pins the exact HyperCast revision whose board run passed, every backport included.

## Scope

One pass across three repos:

1. **HyperCast**: the raw bound rename, `enum` readers in six bindings, `-Wshadow` in the C smoke,
   and any backport HyperTabular's integration finds.
2. **HyperTabular**: the constraints adoption (kernel, C ABI, 11 bindings, corpus, docs),
   `-Wshadow` in the C smoke, the Ruby Fiddle line in `local-clean.sh`.
3. **HyperUuid**: `-Wshadow` in the C smoke, the Ruby Fiddle line in `local-clean.sh`.

Out of scope: constraints on doors HyperCast doesn't check (bool, UUID, the temporal doors);
`pattern` (it waits for the regex crate); any change to HyperTabular's 1.0 exports.

## Part 1 — HyperCast (lands first)

### 1.1 Raw bound rename

`RawConstraints.lower`, `upper`, `exclusive_lower` and `exclusive_upper` become `minimum`,
`maximum`, `exclusive_minimum` and `exclusive_maximum`, matching the OpenAPI keywords as
`multiple_of`, `min_length` and `max_length` already do. Layout, offsets and size (104) are
unchanged; the `HAS_*` bit names stay.

Reaches: `rust/src/abi.rs` and its `offset_of!` asserts; `include/hypercast.h` and the Swift
bundle's copy (`HYPERCAST_BLESS=1 cargo test --test header`); `include/hypercast.hpp`; every
binding's restated layout (C#, Java FFM and JNI, Swift, Go, Python, Ruby, PHP, R's C shim,
TypeScript); `cdylib_smoke.py`, the C smoke, tests and benches; Go's translated core
(`go generate`, then the regeneration check). One CHANGELOG line under Unreleased.

Acceptance: the layout size and offset asserts pass unchanged in value; the header test passes
after blessing; Go's regeneration check passes.

### 1.2 `enum` readers

Python (`.enum`), TypeScript (`get enum`), Ruby, PHP and R read back the declared `enum`. Added:

| Binding | Reader |
|---|---|
| C# | `IReadOnlyList<T>? Enum` |
| Java | `Optional<List<T>> enumValues()` (`enum` is reserved) |
| Swift | ``var `enum`: [T]?`` |
| Go | `Enum() ([]T, bool)` |
| Rust | `Constraints::enum_entries()`, an allocation-free iterator over the declared list |
| C++ | `enum_entries()`, a view over the packed bytes |

(C#, Swift, Java and Go declare entries in their own declaration type, not `T`; their readers
return that type. The implementation plan names each.)

A reader returns the entries as declared: types, values, order and duplicates preserved, absent
when no `enum` is declared. An empty `enum` is a valid declaration (OpenAPI allows it; HyperCast's
corpus expects every value to violate it), so readers distinguish absent from present-but-empty.
Rust's and C++'s views borrow from the `Constraints` they come from and cannot outlive it.

Acceptance, per binding: a round-trip test of a declared list, including a number in a text
`enum` (reads back as declared, never matches) and duplicates; a declared empty `enum` reads back
empty, not absent, and every value violates it. `docs/bindings.md` gains an "`enum` reader" row.
HyperCast's `HAS_ENUM` doc (`rust/src/abi.rs`, and so the header) still calls an empty list a
refused declaration; it is corrected in the same pass.

### 1.3 `-Wshadow`

`c/smoke/run.sh` adds `-Wshadow` to the GCC/Clang flags. Negative control: reintroduce the
`value` shadow in `c/smoke/corpus.cpp` locally and see the smoke fail under Clang.

## Part 2 — HyperTabular kernel and C ABI

HyperTabular's `_with` family (`ColumnSpecWith`, `ColumnBufferWith`, `CellNote`, `RawProblem`,
`hypertabular_*_fill_with`, `hypertabular_faults`) is unreleased and may grow. The 1.0
`ColumnSpec`, `CellVerdict` producers and `fill` exports are untouched and never declare
constraints.

### 2.1 Declaring

`ColumnSpecWith` gains, after `tokens_len`:

- `constraints: *const RawConstraints` — HyperCast's 104-byte layout; null is no constraints, and
  the column casts as it does today.
- `enum_ptr: *const u8`, `enum_len: usize` — the `enum` list in HyperCast's packed format, read
  only under `HAS_ENUM`.

`ColumnBufferWith` gains `checks: *mut RawCheck` — HyperCast's 12-byte layout (`keyword`,
`adjusted`, `enum_index`), one per row.

Every column of a fill call is validated before any cell is read, an empty batch included:
`ERR_CONTRACT`, writing nothing, for a declaration HyperCast's `Constraints::from_raw` refuses,
constraints on a door other than the numeric doors and text, or constraints with a null `checks`.
On a column without constraints, `checks` is never read or written.

### 2.2 Judging a cell

- Delimited cells and a workbook's text cells: HyperCast's checked text cast for the door
  (`cast_checked` / `cast_text_checked`).
- A workbook's typed numeric cell: `checked_from_f64`.
- A declared token, padding, or an uncached formula: `Empty`, no door runs, constraints don't
  apply (`optional_checked`'s rule: blank is absent, never a violation).

A HyperTabular cell's verdict is HyperCast's verdict for that input: the same fault, the same
violation, the same value bits, the same check metadata.

### 2.3 The verdict

- `CellVerdict.reason` `4`: a violation, HyperCast's `cast_checked` code. The value slot holds
  the value exactly as cast, never rounded; `offset`/`len` span the judged text (the cell after
  `trim`; empty for a typed workbook cell).
- `checks[row]`: `keyword` on a violation; `adjusted` and `enum_index` on success; as HyperCast
  writes them. A text `enum` match spans the cell's text; `enum_index` names the declared entry.
- `CellNote` on a violation: `detail` `0`, `origin` as for any cell.

### 2.4 Listing problems

`hypertabular_faults` lists violations at `reason` `4`. `RawProblem` gains `keyword: u32`, read
from `checks` (32 → 36 bytes). A column whose `checks` is null declares no constraints, so lists
no violation.

### 2.5 Proofs

HyperTabular's no-panic link proof passes with the checked paths reachable from its exports. A
failure here that HyperCast's own proof doesn't show is a backport candidate, fixed on the
HyperCast branch.

## Part 3 — HyperTabular's bindings

A HyperTabular binding speaks HyperCast's checked vocabulary unchanged: a column takes the
binding's own HyperCast `Constraints`, a cell hands back the binding's own HyperCast checked
verdict type. HyperTabular defines no new constraint or verdict type.

### 3.1 Declaring

Every column factory for a numeric door or text gains an overload or option taking that
binding's HyperCast `Constraints<T>`, typed to the door, so door and type are matched by
signature at construction (C#: `Column.Int32(ordinal, format, constraints)`). Constraints on any
other door are unrepresentable; where the language allows, a compile-fail test shows it. What
holds the pairing after construction is below.

Ownership: the column keeps its declaration and the packed `enum` alive for the reader's
lifetime.

Rust. Today every factory (`Column::i32` included) returns the same untyped `Column`, and its
`door` is a public field released in 1.0, so a method on `Column` can't pair door and type by
signature, and nothing stops a later `column.door = …`. Making `door` private would break 1.0's
API, so the guarantee comes in two parts:

- Construction, by signature: one typed constructor,
  `Column::checked::<T: CheckedDoor>(ordinal, constraints: Constraints<'_, T>) -> Column`. It
  packs the declaration's `enum` and interns its own copy for the life of the process, as
  `tokens` does, so the caller's `Constraints` and its entries need not outlive the call.
  `CheckedDoor` is a sealed trait implemented for each numeric type and for HyperCast's `Text`,
  each naming its one door. The door comes from `T`, never from the caller, so a mismatched pair
  can't be written. A compile-fail test shows a non-checkable type (`Uuid`, `Date`) is refused.
  Numeric format and tokens chain as on any column (`.format(…)`, `.tokens(…)`).
- After construction, at run time: the column holds the declaration by value — `RawConstraints`,
  its interned packed entries as `&'static [u8]` (as it interns tokens and months), and the door it was typed
  for — and stays `Copy`. When a read is planned, a column whose `door` no longer equals that
  door is refused with the same contract error as a refused declaration, before any cell is
  read. A test mutates `door` after construction and sees the refusal.

Every other binding whose column can be built untyped or changed after construction records the
door its constraints were typed for and refuses the same mismatch at run time; where the
binding's column type is immutable and its factories typed, the signature alone carries it.

### 3.2 Reading

A checked getter beside the plain one, returning the binding's HyperCast checked verdict:

| Binding | Checked verdict |
|---|---|
| C# | `CheckedVerdict<T>` |
| Java | HyperCast's checked sealed interface |
| Swift | HyperCast's checked enum |
| Go | HyperCast's checked result |
| Rust | `Result<Checked<T>, Rejection<T>>` |
| C++ | HyperCast's `noexcept` checked `Verdict` |
| TypeScript | the `ok` / `violation` discriminant |
| Python, Ruby, PHP | HyperCast's checked result objects |
| R | `hypercast_verdict` rows with the keyword column |

- A violation carries the value as cast. Text is the cell's text, or the declared entry when
  `enum_index` matched, looked up in the column's own `Constraints`.
- `adjusted` gives the flagged success; `Optional` / `optional_checked` maps `Empty` to absent.
- On an unconstrained numeric or text column the checked getter keeps the cell's own verdict:
  `Empty`, `Malformed` and `OutOfRange` come through as the door's fault, and a cell that cast
  is a success with `adjusted` false and no `enum` match. Only the absence of check metadata
  comes from the declaration; the getter never reads `checks` for such a column.
- The plain getter on a constrained column is a contract violation (the binding's exception
  idiom): its two-arm verdict would drop the violation.
- Each binding's problem listing gains the violation kind, with the keyword under the binding's
  HyperCast `Keyword` name.

### 3.3 Mechanics

C#, Java, Swift, Go and R's C shim restate and grow `ColumnSpecWith`, `ColumnBufferWith` and
`RawProblem`, with size and offset asserts where the binding has them. Python, Ruby and PHP go
through HyperTabular's Rust extensions, which take HyperCast's Rust `Constraints`. TypeScript goes
through its napi crate; C++ through `include/hypertabular.hpp`.

### 3.4 Docs

HyperTabular's binding table gains "checked column" and "checked getter" rows; every binding
README gains one example, verified by hand; CHANGELOG entries in all three repos.

## Part 4 — Testing, proofs, CI

### 4.1 Shared corpus

`corpus/constraints.json` in HyperTabular, replayed byte for byte by the Rust suite and all 11
bindings (Ruby on both backends, C++ through its smoke), and added to `r_corpus` in
`.github/hyper.json`. Each case is a small CSV, or a cell list built into an XLSX/ODS fixture,
with column declarations and expected verdicts: value, fault, violation and keyword, `adjusted`,
`enum_index`, and the declared entry's text on a match.

Float expectations are encoded as exact bits, so `-0.0` and every value are compared bit for bit;
`checked_from_f64(-0.0)` is expected to deliver `+0.0`, since no door delivers `-0`.

Cases drawn from HyperCast's constraints vectors, plus: tokens, padding and uncached formulas
staying `Empty` under constraints; a quoted cell unescaped into the arena, then trimmed (span
maps back); typed workbook numerics (`-0.0`, `0.1 + 0.2`); a number in a text `enum`; case
folding versus `CASE_SENSITIVE`; duplicate `enum` entries (first wins).

A Rust equivalence test asserts, case by case, that HyperTabular's verdict equals HyperCast's
for the same input along that input's actual path: the checked text cast for a delimited or text
cell, `checked_from_f64` for a typed workbook number.

### 4.2 Contract tests

- At the raw C/kernel boundary: a declaration HyperCast refuses (`multipleOf: "0"`, code 5),
  constraints on an unsupported door, null
  `checks` with constraints — each `ERR_CONTRACT` writing nothing; every column validated before
  any cell, an empty batch included.
- In each binding: unsupported doors unrepresentable (compile-fail where the language allows);
  a door changed after construction refused at run time where the column allows the change
  (Rust's public `door` among them); the plain getter on a constrained column raises the
  contract violation.
- Unconstrained columns: once with no `checks` buffer, once with deliberately stale metadata in
  one (a `keyword`, `adjusted` 1, an `enum_index`). Each run holds a cell that casts, an empty
  cell and a malformed one: the faults come through unchanged, the cast cell is a success with
  `adjusted` false and no `enum` match, and the stale metadata never shows.
- `RawProblem` at 36 bytes with `keyword` at reason 4; layout asserts in every restating binding.

### 4.3 Proofs

HyperTabular: no-panic (with the checked paths reachable), `kernel_allocation_free`,
`kernel_workbook_allocation_free`, and the fault-span invariant over every truncation, extended
to violations. HyperCast: no-panic and its canary, `allocation_free`, `fault_span_invariant`,
`detailed_abi`, rerun after the rename and readers.

### 4.4 Negative controls

Each counts only once seen to fail:

- a corrupted corpus expectation fails every binding's replay;
- a kernel that rounds the violation value fails the equivalence test;
- a getter that reads `checks` on an unconstrained column fails the stale-metadata test;
- the reintroduced shadow fails the C smoke under `-Wshadow`;
- the no-panic canary fails naming only the canary.

### 4.5 Local, then CI

Each repo's full local loop on macOS per its CLAUDE.md (`local-clean.sh`, `local-core.sh`, every
suite and linter), `cargo no-panic` in a Linux container. The report lists macOS results, Linux
results, and what was left to CI (Windows, s390x) separately.

Order: one HyperCast PR (1.1–1.3 plus any backport) → a `purpose=board` run on its head →
HyperTabular's PR pins that exact run and commit → HyperUuid's PR. The owner drives every commit,
push, PR and dispatch; one push per CI round. A HyperCast bug found after its PR merges goes into
a follow-up HyperCast PR before the tag, and the pin moves to it.
