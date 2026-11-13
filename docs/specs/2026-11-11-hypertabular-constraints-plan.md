# HyperTabular constraints adoption — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** HyperTabular declares HyperCast's OpenAPI 3.2 constraints per column and reports HyperCast's
checked verdict per cell in all 11 bindings, with HyperCast's raw bound rename, `enum` readers and
the smoke/clean fixes landed first, so HyperCast 1.1 is left needing only versions, docs and shipping.

**Architecture:** HyperCast changes first (names, readers, flags), in `../HyperCast`. HyperTabular's
unreleased `_with` ABI grows a constraints pointer, a packed `enum` and a per-row `RawCheck` array;
the kernel calls HyperCast's checked functions per cell; every binding passes its own HyperCast
`Constraints` through and returns its own HyperCast checked verdict. A shared corpus and a Rust
equivalence test hold HyperTabular's verdict equal to HyperCast's.

**Tech Stack:** Rust (no_std kernel, cbindgen, no-panic), C/C++, C#, Java (FFM + JNI), Swift, Go
(wasm2go), Python (PyO3), Ruby (Magnus + Fiddle), PHP (ext-php-rs), R (C shim), TypeScript (napi-rs).

**Spec:** `docs/specs/2026-11-11-hypertabular-constraints-design.md` (HyperTabular). Read it first.

## Global Constraints

- The owner drives git. No step commits, pushes, opens a PR or dispatches a workflow; each task ends
  with its changes left in the working tree, verified. "Checkpoint" below means: stop, report the
  diff summary and the commands run.
- Every change reaches every binding with parity in behaviour, API and wording; where one can't
  follow, the report says why.
- Each verification counts only when its negative control was seen to fail.
- HyperCast's `RawConstraints` stays 104 bytes; offsets unchanged.
- No door delivers `-0`; float expectations compare exact bits.
- A violation's value is the value as cast, never rounded.
- `Empty` from a token, padding or an uncached formula never runs a door and is never a violation.
- HyperTabular's 1.0 `ColumnSpec`/`fill` exports are untouched.
- Generated files are regenerated, never hand-edited: `include/*.h` (`*_BLESS=1 cargo test --test header`),
  `go/internal/core/core.go` (`go generate`), `typescript/binding.{js,d.ts}` (`bun run build`),
  `php/stubs/*.stub.php` (`php tools/generate-stubs.php`), `r/inst/AUTHORS` (`r/tools/vendor.sh`).
- Run each repo's `.github/scripts/local-clean.sh` before any local build after a `rust/` change.
- Until HyperCast's next version is on its registries, HyperTabular verifies against `../HyperCast`'s
  working tree through out-of-tree overrides (HyperTabular CLAUDE.md, "Local loop"): cargo
  `[patch.crates-io]` via a scratch `CARGO_HOME`, `go.work` with `replace`, local NuGet feed, Maven
  repository, npm tarballs, pip/R installs. Never write overrides into the tracked tree.
- `docs/superpowers/` in either repo is the owner's; leave it alone.
- Report macOS, Linux (Docker) and CI-only (Windows, s390x) results separately.

## Refinements to the spec found while planning

Each is a naming or mechanism detail inside the approved design; flag at the first checkpoint.

1. HyperCast's Rust `Constraints` already has a `pub(crate) fn entries()`; the public reader is
   `enum_entries()`.
2. C#, Swift, Java and Go declare `enum` entries in their own declaration type, not `T`; each reader
   returns that type (Task 3).
3. `Column` interns packed lists for the process's life (`column.rs` `tokens`), so
   `Column::checked` takes `Constraints<'_, T>`, not only `'static`, interning the packed `enum`.
4. HyperTabular has no fault-span invariant test today; Task 11 creates one.
5. HyperTabular's C smoke already compiles C++ with `-Wshadow` (`c/smoke/run.sh:166`,
   `CMakeLists.txt:38`); HyperUuid's smoke has no warning flags at all, so it gets HyperTabular's
   full set, not `-Wshadow` alone.
6. Rust's `Problem.reason` is `Option<Reason>` and `Reason` has no violation; a violation is
   `reason: None`, `keyword: Some(_)`, `plan_column: Some(_)`.
7. Python, Ruby, PHP, R and TypeScript can pair any door with any constraints object, so they
   hold the pairing at run time: a mismatch is a contract error when the plan is bound (Tasks
   17–21), not a typed factory.
8. An empty `enum` is valid (HyperCast retired that refusal; code 9 is `EnumNotUtf8`). Readers
   distinguish absent from present-but-empty; contract tests use `multipleOf: "0"` (code 5);
   HyperCast's stale `HAS_ENUM` doc is fixed in Task 1. The spec is corrected to match.

## Review Focus

1. A quoted cell (`"  a""b  "`) unescaped into the arena, then trimmed: the violation span must index
   the cell's text, and a text `enum` match must still deliver the declared entry — Task 7.
2. A typed workbook number at the text door with a text `enum` (`42` vs `["42"]`): judged on the
   canonical text the 1.0 path already produces, so it matches — Task 8.
3. A `multipleOf` rounding that overflows the door (`i8`, input `127`, `multipleOf: "5"`, ceiling —
   HyperCast's own vector): violation `multipleOf`, value 127 — Task 10 corpus.
4. Two columns over the same ordinal, one constrained and one not: each keeps its own verdict and
   only the constrained one writes `checks` — Task 6.
5. A batch whose `checks` array is shorter than `rows` is a caller bug the kernel can't see; each
   binding sizes `checks` from the same row count as `verdicts` — asserted in Task 13's C# test and
   mirrored per binding.

---

## Part 1 — HyperCast (`../HyperCast`, branch the owner names)

### Task 1: Rename the raw bounds

**Files** (from the survey; grep `\b(exclusive_)?(lower|upper)\b` and the per-language spellings to
confirm nothing is missed):
- Modify: `rust/src/abi.rs:94-98,334-338,365-386,414-418`; `rust/src/checked.rs:439-450,912-915`;
  `rust/src/constraints.rs:412-424,856-859` and tests `1023-1314`; `rust/src/ffi.rs:654-658`;
  `rust/src/tests.rs` (not `3620/3622`); `rust/src/python_ext.rs:1694-1910`;
  `rust/src/ruby_ext.rs:966-970`; `rust/src/php_ext/constraints.rs:351-354,443`;
  `rust/tests/{conformance,detailed_abi,fault_span_invariant}.rs`; `rust/benches/detailed_abi.rs:36-44`
- Modify: `include/hypercast.hpp:2354-2359`; `csharp/HyperCast/Interop/RawValues.cs:348-425`,
  `csharp/HyperCast/Constraints{T}.cs:102-149`, `csharp/HyperCast.Tests/{Interop,Constraints,Corpus}Tests.cs`;
  `java/jni/src/lib.rs:1096-1183`, `java/jni/src/tests.rs`; `go/interop.go:136-180,684-688`,
  `go/checked.go`, `go/core.go:524-528`, `go/checked_test.go`; `r/src/hypercast.c:1062-1066,1214-1219`;
  `typescript/src/lib.rs`; `.github/scripts/cdylib_smoke.py`; `c/smoke/main.c`;
  READMEs `csharp/README.md:929`, `go/README.md:614`, `c/README.md:133`; `CHANGELOG.md`
- Regenerate: `include/hypercast.h` + Swift bundle copy; `go/internal/core/core.go` (`go generate`)

**Interfaces:** Produces `RawConstraints { size, flags, rounding, min_length, max_length, _reserved,
minimum, maximum, multiple_of, exclusive_minimum, exclusive_maximum }`; C#/Go fields `Minimum`,
`Maximum`, `ExclusiveMinimum`, `ExclusiveMaximum`; JNI `WireConstraints` the same snake_case.

- [ ] **Step 1:** Rename in `abi.rs` only, run `cargo test` in `rust/`: expect compile failures listing
  every Rust site (the failing control).
- [ ] **Step 2:** Rename every Rust site; `cargo test` passes; `offset_of!` asserts unchanged in value.
- [ ] **Step 3:** `HYPERCAST_BLESS=1 cargo test --test header`, then `cargo test --test header` passes.
- [ ] **Step 4:** Rename every binding and smoke site listed; `cargo cdylib`; run C#, Java (FFM + JNI),
  Go, R, TypeScript, Python, Ruby (both backends), PHP suites per HyperCast CLAUDE.md "Local loop",
  `python3 .github/scripts/cdylib_smoke.py`, and the C smoke (`SMOKE_CXX=1 c/smoke/run.sh`).
- [ ] **Step 5:** `go generate` in `go/`; `HYPERCAST_CHECK_CORE=1 go test -count=1 -run 'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .` passes.
- [ ] **Step 6:** `grep -rnE '\b(exclusive_)?(lower|upper)\b|ExclusiveLower|ExclusiveUpper|\.Lower\b|\.Upper\b'`
  across the tracked tree (excluding prose hits) returns nothing raw-layout related.
- [ ] **Step 6b:** Fix the stale `HAS_ENUM` doc in `rust/src/abi.rs:396-397` (an empty list is a
  valid declaration every value violates; not a refusal), re-bless the header, and fix any copy
  of that sentence in binding docs (`grep -rn "check_constraints.*refuses\|empty .*enum.*refus"`).
- [ ] **Step 7:** CHANGELOG Unreleased, `### Changed`: "**`RawConstraints`' bounds take OpenAPI's
  names.** `lower`, `upper`, `exclusive_lower` and `exclusive_upper` are `minimum`, `maximum`,
  `exclusive_minimum` and `exclusive_maximum`, in the header and every binding that restates the
  layout; the layout is unchanged." Checkpoint.

### Task 2: `enum` readers in Rust and C++

**Files:**
- Modify: `rust/src/constraints.rs` (beside `raw()` at :527), `rust/src/no_panic_harness.rs`,
  `include/hypercast.hpp` (class `Constraints`, :2214)
- Test: `rust/tests/constraints_raw.rs`, `c/smoke/main.cpp`

**Interfaces:** Produces
- `impl<'e, T: Numeric> Constraints<'e, T> { pub fn enum_entries(&self) -> Option<EnumNumbers<'e>> }`
  with `pub struct EnumNumbers<'e>: Iterator<Item = Decimal> + ExactSizeIterator + Clone`
- `impl<'e> Constraints<'e, Text> { pub fn enum_entries(&self) -> Option<EnumTexts<'e>> }` with
  `pub struct EnumTexts<'e>: Iterator<Item = &'e [u8]> + ExactSizeIterator + Clone`
- C++: `std::optional<EnumView<T>> Constraints<T>::enum_entries() const noexcept`, `EnumView<T>` a
  forward range yielding `Decimal` for numeric `T`, `std::string_view` for text; it borrows the
  `Constraints`' storage.
- Both return `None`/`nullopt` exactly when `HAS_ENUM` is clear; both walk `Entries` (all five
  variants: `Decimals`, `Text`, `PackedDecimals`, `PackedText`, `None`).

- [ ] **Step 1: Failing tests** in `constraints_raw.rs`:
  `enum_entries_read_back_as_declared` — `Constraints::<i32>::numeric(rules, Some(&[d("20"), d("10"), d("20")]))`
  → `enum_entries().unwrap().collect::<Vec<_>>() == [d("20"), d("10"), d("20")]`;
  `text_enum_entries_read_back_as_declared` — `["Male", "male", "Male"]` reads back in order with
  duplicates; `from_raw_packed_entries_read_back` for both packed variants, a number-in-text entry
  reading back as `b""`; `no_enum_reads_none` (`HAS_ENUM` clear → `None`);
  `an_empty_enum_reads_back_empty` — `from_raw` with `HAS_ENUM` and `entries: &[]` is `Ok`, its
  `enum_entries()` is `Some` with `len() == 0`, and `cast_checked("7", …)` is an `enum` violation.
- [ ] **Step 2:** `cargo test --test constraints_raw` fails (no `enum_entries`).
- [ ] **Step 3:** Implement on top of `Entries::{len, number, text}`; add `#[no_panic]` wrappers for
  both readers (and one `next()` each) in `no_panic_harness.rs`.
- [ ] **Step 4:** Tests pass; `cargo no-panic` in a Linux container passes, and the canary
  (`python3 ../HyperForge/.github/scripts/no_panic_canary.py --name hypercast_canary --root rust -- cargo no-panic`)
  fails naming only `hypercast_canary`.
- [ ] **Step 5:** C++ `EnumView`; in `main.cpp` add a round-trip check for an `int32_t` list with a
  duplicate and a text list; `SMOKE_CXX=1 c/smoke/run.sh` passes. Checkpoint.

### Task 3: `enum` readers in C#, Java, Swift, Go; docs row

**Files:**
- Modify: `csharp/HyperCast/Constraints{T}.cs` (store the declared list beside `_entries`, :50);
  `java/src/main/java/io/github/skunkwerkx/hypercast/Constraints.java` (Builder keeps the declared
  `String[]`/`BigDecimal[]`); `swift/Sources/HyperCast/Constraints.swift`; `go/checked.go`
  (`declaration` keeps `Rule.entries` and `Rule.numbers`)
- Test: `csharp/HyperCast.Tests/ConstraintsTests.cs`, `java/src/test/.../ConstraintsTest.java`,
  `swift/Tests/HyperCastTests/CastTests.swift`, `go/checked_test.go`
- Modify: `docs/bindings.md` (new row after :167), each binding's README constraints section,
  `CHANGELOG.md`

**Interfaces:** Produces
- C#: `public IReadOnlyList<ConstraintValue>? Enum { get; }`
- Java: `public Optional<List<Object>> enumValues()` — an unmodifiable list of the `String`s or
  `BigDecimal`s as passed to `enumOf`
- Swift: ``public var `enum`: [ConstraintValue]? { get }``
- Go: `func (c Constraints[V]) Enum() (entries []string, numbers bool, ok bool)` — `entries` as
  `constraintText` rendered them, `numbers` true when they were declared as numbers

- [ ] **Step 1: Failing tests**, one per binding, same three cases and names in each idiom:
  `EnumReadsBackAsDeclared` (numeric door, `[20, 10, 20]`, order and duplicate kept),
  `ANumberInATextEnumReadsBackAsDeclared` (text door, entries `["a", 1]` or the binding's overload:
  reads back the number, never matches), `NoEnumReadsAbsent`, and `AnEmptyEnumReadsBackEmpty`
  (declared empty: the reader gives an empty list, not absent, and a cast of `7` violates `enum`).
- [ ] **Step 2:** Run each suite; the new tests fail to compile or fail.
- [ ] **Step 3:** Implement each reader returning the stored declaration (a copy or read-only view;
  never the packed bytes).
- [ ] **Step 4:** Each suite and its linters pass (`dotnet test`/format check, `./gradlew test testFloor
  checkSuiteParity javadoc spotlessCheck`, `swift test` + `swift format lint`, `go test ./...` +
  `go vet`).
- [ ] **Step 5:** `docs/bindings.md` row "`enum` reader": Rust `enum_entries()`, C# `Enum`, Java
  `enumValues()`, Go `Enum()`, Swift `` `enum` ``, PHP `$enum`, Ruby `#enum`, Python `.enum`,
  TypeScript `enum`, R `$enum`, C++ `enum_entries()`. README examples verified by running them.
  CHANGELOG `### Added`: "**Every binding reads a declared `enum` back.** …" with one bullet per
  binding added. Checkpoint.

### Task 4: `-Wshadow` and `local-clean.sh` in the siblings

**Files:**
- Modify: HyperCast `c/smoke/CMakeLists.txt:31` (add `-Wshadow`), `c/smoke/run.sh:164,168,206`
  (a `cxx_warnings=(-Wall -Wextra -Wpedantic -Wshadow -Werror)` array as HyperTabular's `run.sh:166`)
- Modify: HyperUuid `c/smoke/CMakeLists.txt` (HyperTabular's `_warnings` block, `CMakeLists.txt:36-38`,
  applied as at its `:120`), HyperUuid `c/smoke/run.sh` C++ lines if any
- Modify: HyperTabular and HyperUuid `.github/scripts/local-clean.sh`: HyperCast's lines 52–58 (the
  flat `*_native.so`/`.bundle` and Fiddle's `native/*/`), each path checked against that repo's
  `.gitignore` (HyperUuid: `.gitignore:53,120-121`; its tracked `native/README.md` survives the
  `*/` glob)

- [ ] **Step 1 (negative control):** In HyperCast, temporarily rename `want_value` back to `value`
  in `c/smoke/corpus.cpp:1000`; with the new flags `SMOKE_CXX=1 c/smoke/run.sh` fails on the shadow.
  Revert; it passes.
- [ ] **Step 2:** HyperUuid: add the warning set; its smoke passes (fix any warning it surfaces, in
  its smoke sources only); the same temporary shadow in one of its smoke `.cpp` files fails it.
- [ ] **Step 3:** In each sibling, stage a dummy `ruby/lib/<name>/native/x/` dir and flat `.so`;
  `local-clean.sh` removes them and names the rebuild; HyperUuid's `native/README.md` remains;
  `git status` clean afterwards.
- [ ] **Step 4:** CHANGELOG lines only where the repo logs build-tooling changes. Checkpoint.

---

## Part 2 — HyperTabular kernel and C ABI

Before Task 5: point HyperTabular's Rust at `../HyperCast/rust` through a scratch `CARGO_HOME`
with `[patch.crates-io] hypercast = { path = "…/HyperCast/rust" }`; `cargo test` in `rust/` passes
unchanged (baseline).

### Task 5: Layouts

**Files:**
- Modify: `rust/src/kernel/abi.rs` (`ColumnSpecWith` :825, `ColumnBufferWith` :887, `RawProblem`
  :733, `pin_layout!` :48-70, `offset_of!` :87-95, :155-168); regenerate `include/hypertabular.h`
- Test: `rust/tests/kernel_delimited.rs` (layout section)

**Interfaces:** Produces
- `ColumnSpecWith { …, tokens, tokens_len, constraints: *const RawConstraints, enum_ptr: *const u8, enum_len: usize }`,
  size `120 + 7 * WORD`; `ColumnSpecWith::new` sets the three to null/0
- `ColumnBufferWith { size, values, verdicts, notes, checks: *mut RawCheck }`, size `5 * WORD`
- `RawProblem { …, found, keyword: u32 }`, `SIZE = 36`
- `CellVerdict` doc: `reason` `4` a violation; a `pub const VIOLATION: u32 = 4` on `CellVerdict`
- re-export `hypercast::{RawConstraints, RawCheck}` from `kernel::abi`

The `_with` exports match `size` exactly (`exports.rs:345-346,848-849,1022`), so every caller
that builds these structs must grow with them in this task, the new fields null/zero, or its suite
breaks until its own binding task. This task ends with every suite green and behaviour unchanged.

- [ ] **Step 1:** Update the `pin_layout!`/`offset_of!` asserts first; build fails (control).
- [ ] **Step 2:** Grow the structs; update every Rust struct literal of `ColumnSpecWith`,
  `ColumnBufferWith` and `RawProblem` (`grep -rn 'ColumnSpecWith {\|ColumnBufferWith {\|RawProblem {'`
  over `rust/`, `typescript/src`, `java/jni/src`) with null/`0`; asserts pass;
  `HYPERTABULAR_BLESS=1 cargo test --test header`, then `cargo test --test header` passes.
- [ ] **Step 3:** Grow every restated layout with null/zero fields and its size/offset asserts: C#
  `Native.cs:189,213,247` + `NativeTests.cs:226-241`; Java `Abi.java:157-200` + `NativeTest.java:289-312`;
  Go `core.go:85-100`, `putSpecWith` :622, `tabular.go:127,196-197`, then `go generate`; Swift
  `Columns.swift:51-53` + `NativeTests.swift:239-255`; R `hypertabular.c:88-117,121,228-281`; Ruby
  Fiddle `runtime/columns.rb:9-19`, `PROBLEM_BYTES` (`batch.rb:197`, `runtime.rb:206`); C++
  `hypertabular.hpp:983,997`.
- [ ] **Step 4:** `cargo cdylib`; every binding's suite passes unchanged (the full local loop per
  HyperTabular CLAUDE.md), and `check_exports.py` passes. Checkpoint.

### Task 6: Validation before any cell

**Files:**
- Modify: `rust/src/kernel/delimited/fill.rs` (`honours` :1630, `fill_with` pre-read loop :1667-1681,
  `Resolved::of` :1612); `rust/src/kernel/workbook/rows.rs` (`fill_with` loop :1677-1691);
  `rust/src/kernel/exports.rs` (doc comments of `hypertabular_delimited_fill_with`,
  `hypertabular_workbook_fill_with`)
- Test: `rust/tests/kernel_delimited.rs`, `rust/tests/kernel_workbook_rules.rs`

**Interfaces:** Produces `pub(crate) fn resolved_constraints(spec: &ColumnSpecWith, door: Door) -> Result<Option<Checks<'_>>, ()>`
in a new `rust/src/kernel/checked.rs`, where `Checks<'a>` is a `Copy` enum holding a
`hypercast::Constraints<'a, T>` per checkable door (`I8`…`U64`, `F32`, `F64`, `Decimal`, `Text`).
`Err(())` maps to `ERR_CONTRACT`.

- [ ] **Step 1: Failing tests** (raw ABI, both fills): `a_refused_declaration_is_a_contract_error`
  (`multipleOf: "0"` at `i32`, HyperCast's code 5) — returns `ERR_CONTRACT`, verdicts/values/cursor untouched (fill
  them with `0xAA` first and compare); `constraints_on_an_unchecked_door_are_a_contract_error`
  (`Uuid`, `Date`, `Bool`); `constraints_without_checks_are_a_contract_error`;
  `every_column_is_judged_before_any_cell` (column 0 fine, column 1 refused: column 0's verdicts
  still `0xAA`); `an_empty_batch_still_judges_declarations` (`rows` cap 0, refused declaration →
  `ERR_CONTRACT`); `two_columns_over_one_ordinal_keep_their_own_verdicts` (Review Focus 4: the
  unconstrained twin's `checks` pointer is null and the constrained one's verdicts differ).
- [ ] **Step 2:** They fail.
- [ ] **Step 3:** Implement in both pre-read loops; on an unconstrained column `checks` is never read.
- [ ] **Step 4:** Pass. Checkpoint.

### Task 7: Delimited per-cell judgement

**Files:**
- Modify: `rust/src/kernel/delimited/fill.rs` (`fill_with` cast loop :1730-1740; a `cast_checked_column`
  beside `cast_noted` :852 that writes value, verdict, note and check per row; `rejudge` :1279 for
  text validity); `rust/src/kernel/checked.rs`
- Test: `rust/tests/kernel_delimited.rs`

**Interfaces:** Consumes `Checks<'_>` (Task 6). Uses `hypercast::cast_checked`, `cast_text_checked`,
and the `Rejection`/`Checked` fields; writes `RawCheck { keyword, adjusted, enum_index }` exactly as
HyperCast's `cast_checked` export does (`enum_index = u32::MAX` when none).

- [ ] **Step 1: Failing tests**: `a_violation_keeps_the_value_as_cast` (`i32`, `maximum: "10"`, cell
  `11` → reason 4, value 11, `keyword` 3, span `[0,2]`); `multiple_of_rounding_flags_adjusted`
  (`multipleOf: "0.5"`, midpoint, `1.3` → ok, 1.5, `adjusted` 1); `a_trimmed_text_spans_the_judged_text`
  (`trim`, `maxLength: 1`, cell `"  ab "` → violation, span `[2,2]`);
  `a_quoted_cell_in_the_arena_maps_its_span` (Review Focus 1: `"  a""b  "` with `trim`,
  `enum: ["a\"b"]` → ok, `enum_index` 0; the same with `maxLength: 1` → violation spanning `a"b`
  in the unescaped text); `tokens_and_padding_stay_empty_under_constraints` (`minLength: 3`; a
  token cell and a short row → reason 1, origin token/padded, `checks` row untouched);
  `a_number_in_a_text_enum_never_matches`; `case_folding_unless_case_sensitive`;
  `duplicate_enum_entries_first_wins`.
- [ ] **Step 2:** Fail. **Step 3:** Implement. **Step 4:** Pass; the 1.0 `fill` suite unchanged.
  Checkpoint.

### Task 8: Workbook per-cell judgement

**Files:**
- Modify: `rust/src/kernel/workbook/cell.rs` (`cast_with` :902, `numeric_with` :780, `put_with` :873),
  `rust/src/kernel/workbook/rows.rs` (`write_with` :1138)
- Test: `rust/tests/kernel_workbook_rules.rs`, fixtures via `rust/tests/support/workbook.rs`

**Interfaces:** Typed numeric cells → `hypercast::checked_from_f64`; text cells and a typed cell at
the text door (its canonical text, as the 1.0 path renders it) → the checked text cast.

- [ ] **Step 1: Failing tests**: `a_typed_negative_zero_is_positive_zero` (`f64`, typed `-0.0`,
  `minimum: "0"` → ok, bits `0x0000000000000000`); `a_typed_sum_is_judged_on_its_shortest_digits`
  (`0.30000000000000004`, `maximum: "0.3"` → violation, empty span); `a_typed_number_at_the_text_door_matches_its_text`
  (Review Focus 2: `42`, `enum: ["42"]` → ok, index 0); `an_uncached_formula_stays_empty`;
  `a_boolean_cell_at_a_numeric_door_keeps_its_note` (as 1.0, no `checks` write).
- [ ] **Step 2–4:** Fail, implement, pass. Checkpoint.

### Task 9: `hypertabular_faults` lists violations

**Files:** Modify `rust/src/kernel/faults.rs` (`cell()` :117, literal :139),
`rust/src/kernel/exports.rs:380-440` (doc and the column's `checks` read). Test:
`rust/tests/kernel_delimited.rs` faults section (:4765-5143).

- [ ] **Step 1:** Failing test `faults_list_a_violation_with_its_keyword` (reason 4, `keyword` 3,
  `detail` 0, span) and `a_column_without_checks_lists_no_violation`.
- [ ] **Step 2–4:** Fail, implement, pass. Checkpoint.

### Task 10: Corpus and the equivalence test

**Files:**
- Create: `corpus/constraints.json`; Modify `corpus/README.md` (a "Constraints" section),
  `rust/examples/corpus.rs` if it generates (it writes `delimited.json`/`workbook.json`; extend it
  to write `constraints.json` and its workbook fixtures under `corpus/workbook/`)
- Create: `rust/tests/corpus_constraints.rs` (replay + `every_corpus_key_is_read`) and
  `rust/tests/constraints_equivalence.rs`; register both
- Modify: `.github/hyper.json` `r_corpus` (add `constraints.json` and any new workbook packages)

**Format:** a case is a delimited (`input`, `dialect`) or workbook (`file`, `format`, `sheet`) case
as today, whose `plan` entries may carry `constraints` in HyperCast's corpus shape
(`{"minimum": "10", "multipleOf": "0.5", "rounding": "midpoint", "trim": true, "enum": [...]}`
— copy the key names from `../HyperCast/corpus/constraints.json`), and whose cells may expect
`"violation"` with `keyword` (HyperCast's names), plus `adjusted` and `enum_index` on `ok`. Float
values carry `"bits": "0x…"` beside `value`.

- [ ] **Step 1:** Write the cases: at least one per HyperCast keyword per door family, every Task 7/8
  case, Review Focus 3 (`i8`, input `127`, `multipleOf: "5"`, ceiling → violation `multipleOf`, value
  127, span `[0,3]`, as HyperCast's vector), and an empty `enum` at `i32` (`7` → violation `enum`).
- [ ] **Step 2:** `corpus_constraints` replay passes; corrupt one `keyword` → it fails (control).
- [ ] **Step 3:** `constraints_equivalence`: for each cell, the oracle derives the routing itself
  from the fixture and the declaration — never from the batch — then asks HyperCast, then asserts
  the batch's verdict *and* its reported `origin` match:
  - a cell whose exact unquoted bytes are a declared token (the column's or dialect's list), a cell
    past the row's end, or a workbook formula with no cached value (from the fixture's cell list):
    expect `Empty` with origin token / padded / uncached and no `checks` write — no HyperCast call;
  - a delimited cell or a workbook text cell: `cast_checked` / `cast_text_checked` on the cell's
    text after unescaping (`Batch::raw`'s bytes);
  - a typed workbook number at a numeric door: `checked_from_f64` on its `f64`;
  - a typed workbook number at the text door: render it as the 1.0 text path does
    (`hypercast::shortest_digits`, as `kernel/workbook/number.rs` uses), then `cast_text_checked`;
  - a typed boolean or other cell the door has no twin for: expect the 1.0 path's fault and note.
  Then assert origin, value bits, reason, span, keyword, `adjusted`, `enum_index` equal the
  batch's. Controls: make Task 7's writer round the violation value → fails; make the kernel
  report a padded cell as `ORIGIN_CELL` → fails.
- [ ] **Step 4:** Add the key to every binding's unknown-key list later in its own task. Checkpoint.

### Task 11: Proofs

**Files:** Modify `rust/tests/kernel_allocation_free.rs`, `kernel_workbook_allocation_free.rs` (a
constrained column in each, `enum` included); Create `rust/tests/violation_span_invariant.rs`
(register in `rust/Cargo.toml`).

- [ ] **Step 1:** `violation_span_invariant`: for every case of `corpus/constraints.json` (Task 10)
  and every truncation of its input, every verdict with reason 1–4 satisfies `offset + len <=
  cell text length`. Seen to fail by temporarily offsetting the span by one in Task 7's writer.
- [ ] **Step 2:** Allocation tests with constraints pass (zero allocations).
- [ ] **Step 3:** Mac: `RUSTFLAGS="" cargo rustc --release --lib --crate-type cdylib --features no-panic
  --target-dir target/no-panic-mac -- -C link-arg=-Wl,--no-undefined` passes; Linux container
  `./check-core.sh` passes; `RUSTFLAGS="--cfg hypertabular_canary"` fails on `hypertabular_canary`
  alone. A failure that names a HyperCast checked path is a backport: fix it in `../HyperCast`
  (Task 1–3's branch) with a HyperCast test, rerun HyperCast's proofs, then return here.
  Checkpoint.

---

## Part 3 — HyperTabular Rust API and bindings

### Task 12: Rust public API

**Files:** Modify `rust/src/column.rs`, `rust/src/batch.rs`, `rust/src/kept.rs`, `rust/src/lib.rs`,
`rust/src/error.rs` (doc of `Error::Plan`). Tests: `rust/tests/column_values.rs`, a `trybuild`-style
compile-fail test if the crate already uses one, else a `compile_fail` doctest on `Column::checked`.

**Interfaces:** Produces
- `pub trait CheckedDoor: sealed::Sealed { const DOOR: Door; }` for `i8`…`u64`, `f32`, `f64`,
  `Decimal`, and HyperCast's `Text` marker — declaration typing only; it is not a `Value` and
  the getters don't take it (`get_checked<T: Value>` for numbers, `text_checked` for text)
- `pub fn checked<T: CheckedDoor>(ordinal: usize, constraints: Constraints<'_, T>) -> Column`
  (interns the packed `enum` as `tokens` does; records `T::DOOR`)
- `Column::spec_with` → `Err(Error::Plan { column })` when `door != ` the recorded door
- `Batch::get_checked<T: Value>(&self, column, row) -> Result<Checked<T>, Rejection<T>>`,
  `Batch::text_checked(&self, column, row) -> Result<Checked<&'r str>, Rejection<&'r str>>`,
  `Row::get_checked`, `Row::text_checked`
- `Batch::get`/`text` on a constrained column panic, as `get` does for a type the door doesn't make
- `Problem { …, pub keyword: Option<Keyword> }`; `KeptFaults` carries it

- [ ] **Step 1: Failing tests**: `a_checked_column_reads_violations`;
  `a_mutated_door_is_refused_before_any_cell` (`let mut c = Column::checked(0, i32s); c.door = Door::Uuid;`
  → `Error::Plan { column: 0 }`); `the_plain_getter_panics_on_a_constrained_column` (`#[should_panic]`);
  `an_unconstrained_column_keeps_its_faults_under_get_checked` — run twice, once normally and once
  with the batch's `checks` storage pre-filled with stale `RawCheck { keyword: 3, adjusted: 1, enum_index: 0 }`:
  a cast cell is `Ok(Checked { adjusted: false, enum_index: None, .. })`, an empty cell `Err(Fault(Empty))`,
  a malformed one `Err(Fault(Malformed))`; `a_text_enum_match_delivers_the_declared_entry`
  (`"MALE"` → `"Male"`); `optional_checked_maps_empty_to_none`;
  compile-fail: `Column::checked(0, Constraints::<Uuid>…)` does not compile.
- [ ] **Step 2–4:** Fail, implement, pass; `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`. Checkpoint.

### Tasks 13–22: one task per binding

Each binding task has the same shape; the table gives its files and names. **Shared steps:**

- [ ] **Step 1: Failing tests**, named in the binding's idiom after these:
  `ACheckedColumnReadsAViolation`, `ARoundedValueIsFlaggedAdjusted`,
  `ATextEnumMatchDeliversTheDeclaredEntry`, `UnconstrainedCheckedGetterKeepsFaults` (cast, empty and
  malformed cells; once with no `checks` buffer, once with stale metadata injected where the binding
  owns the buffer), `ThePlainGetterRefusesAConstrainedColumn` (the binding's contract-violation
  idiom), `AMutatedDoorIsRefused` (only where the column can change after construction),
  `AnUncheckableDoorIsUnrepresentable` (compile-fail where the language has it; otherwise omit and
  say so), `FaultsListAViolationWithItsKeyword`, `ChecksIsSizedLikeVerdicts` (Review Focus 5),
  layout asserts for every restated struct (`ColumnSpecWith`, `ColumnBufferWith`, `RawProblem` 36).
- [ ] **Step 2:** Corpus replay reads `corpus/constraints.json`; add `constraints`, `violation`,
  `keyword`, `adjusted`, `enum_index`, `bits` to the binding's known-key list; corrupting one
  expectation fails the replay (control, once per binding).
- [ ] **Step 3:** Implement; the binding's full suite and linters pass; generated files regenerated.
- [ ] **Step 4:** README: one example, run by hand. Checkpoint.

| Task | Binding | Declare | Checked getter | Restated layouts / problems | Build + test |
|---|---|---|---|---|---|
| 13 | C# | `Column.cs:63` overloads `Column.Int32(int ordinal, NumFormat format, Constraints<int> constraints)` … `Column.Text(int ordinal, Constraints<string> constraints)`; `Columns.cs:82,127,168` pins packed bytes | `Batch.GetChecked<T>(int column, int row) : CheckedVerdict<T>`, `Row.GetChecked<T>` | `Native.cs:189,213,247`; asserts `NativeTests.cs:226-241`; `Problem` gains `Keyword?` | local NuGet feed of `../HyperCast`; `dotnet test`, format check, AOT smoke |
| 14 | Java | `Column.java` factories taking `Constraints<T>`; `Columns.java:120+` | `Batch.getChecked(int, int, Class<T>) : CheckedVerdict<T>` (HyperCast's sealed type), `Row.getChecked` | `Abi.java:157-200`; asserts `NativeTest.java:289-312`; `Problem.keyword()` | local Maven repo; `./gradlew test testFloor checkSuiteParity javadoc spotlessCheck` (JNI lib after `cargo cdylib`) |
| 15 | Swift | `Column.swift:62` factories with `constraints: Constraints<T>` | `Batch.checked(_:row:as:) -> CheckedVerdict<T>`, `Row` twin | header types; `Columns.swift:51-53`; asserts `NativeTests.swift:239-255`; `Problem.keyword` | `.github/scripts/local-core.sh`, `swift test`, `swift format lint` |
| 16 | Go | `column.go:111` `Checked[V](ordinal int, c hypercast.Constraints[V]) Column` (unexported fields; immutable) | `GetChecked[T](b *Batch, col, row int) hypercast.Checked[T]` (HyperCast's checked result), `CellChecked[T]` on `Row` | `core.go:85-100`, `putSpecWith` :622, `tabular.go:127,196-197`; `Problem.Keyword` | `go.work` replace; `go generate`; `go test ./...`; core regeneration check; wasm legs |
| 17 | Python | `__init__.py:463` dataclass gains `constraints: hypercast.Constraints | None`; `python_ext.rs:1448` `column_of` | `Batch.get_checked`, `Row.get_checked` (`python_ext.rs:781,1055`); stubs `_native.pyi` | Rust API; `Problem.keyword` (`__init__.py:801`) | pip install of `../HyperCast/python`; `maturin develop --release`; `pytest`; ruff/mypy |
| 18 | Ruby | `column.rb:33` `Data.define` member `constraints`; `packed_with` :214, `SPEC_WITH_BYTES` :269 | `Batch#get_checked`, `Row#get_checked` (both backends: `ruby_ext.rs` `Spec::with` :251 and Fiddle `runtime/columns.rb`) | `runtime/columns.rb:9-19`, `PROBLEM_BYTES` `batch.rb:197`, `runtime.rb:206`; `agreement.rb` covers both | `bundle exec rake native:dev`; `rspec` on both backends; rubocop |
| 19 | PHP | `Column` (`php_ext/values.rs:636`) gains `?HyperCast\Constraints $constraints`; `plan.rs:25` | `Batch::getChecked`, `Row::getChecked` (`php_ext/batch.rs:814,909`) → HyperCast's checked verdict | Rust API; `Problem->keyword` | `cargo php-ext`; `composer test` (both extensions); stubs regenerated, `HYPERTABULAR_CHECK_STUBS=1 composer test` |
| 20 | R | `columns.R:118` `new_column(..., constraints = NULL)`; `col_*` gain `constraints =` taking `hypercast_constraints()` | no per-cell getter: a constrained column's frame is HyperCast's checked `hypercast_verdict` rows with the `keyword` column; an unconstrained one is unchanged | C shim `hypertabular.c` `PACKED_SPEC` :121, builder :228-281, `_Static_assert` :88-117; `problems()` gains `keyword` | `r/tools/vendor.sh` (`HYPERCAST_PATH=../HyperCast/rust`); `devtools::test("r")`; `R CMD check --as-cran`; lintr |
| 21 | TypeScript | napi `Column` (`lib.rs:1464`) factories take an optional `constraints: Constraints` (no `withConstraints` builder, so a column's constraints are fixed with its door) | `Batch.getChecked`, `Row.getChecked` (`lib.rs:3168,3286`) → the `ok`/`violation` discriminant | Rust API; `Problem.keyword` | npm tarball of `../HyperCast/typescript`; `bun run build`, `bun run test`, `test:bun`, `lint`; `build:wasi` + `NAPI_RS_FORCE_WASI=error bun run test` |
| 22 | C / C++ | `hypertabular.hpp:624` `Column::checked(ordinal, const hypercast::Constraints<T>&)` | `Batch::get_checked<T>` (:1352) → HyperCast's checked `Verdict`; `Row` twin | `detail::spec` :997, `static_assert` :983; `faults()` gains keyword; C smoke `main.c` sets the raw fields | `c_package.py assemble`, `SMOKE_CXX=1 c/smoke/run.sh`; `c/smoke/corpus.cpp` replays `constraints.json` |

Where the door/type pairing is held, per binding:
- By signature alone (factories typed, columns immutable): C#, Java, Swift, Go (unexported
  fields). `AMutatedDoorIsRefused` is omitted there and the report says so.
- At run time, because the language can pair any door with any constraints object: Python, Ruby,
  PHP, R, TypeScript. The constraints object knows its door (HyperCast's `Constraints` carries it);
  a column whose door differs is refused as a contract error when the plan is bound, and
  `AMutatedDoorIsRefused` becomes `AMismatchedDoorIsRefused` (build one directly with a mismatched
  pair).
- Rust (Task 12) and C++ (Task 22): typed construction, plus the run-time check, since Rust's `door`
  is public and Task 22 checks whether C++'s `Column` exposes its door for change (if it does, the
  test applies; if not, it's omitted and said so).

### Task 23: Docs, exports, changelogs

**Files:** HyperTabular `docs/design.md` ("The bindings, side by side" :603, mapping table :659;
"Verdicts and fault spans" :246; "Problems" :258): rows "checked column", "checked getter",
"violation in problems"; `README.md`; `CHANGELOG.md` `### Added` ("**A column declares
HyperCast's constraints.** …" with per-binding bullets and a corpus bullet); `.github/scripts/check_exports.py`
passes (no new export; layouts only); HyperCast's CHANGELOG already updated (Tasks 1–3).

- [ ] **Step 1:** Write; verify every README example by running it.
- [ ] **Step 2:** `python3 .github/scripts/check_exports.py` and `python3 -m pytest .github/scripts/tests`
  pass in all three repos. Checkpoint.

### Task 24: Whole verification and report

- [ ] **Step 1:** In each repo: `local-clean.sh`, `local-core.sh`, every suite and linter from its
  CLAUDE.md, every negative control in Tasks 1–22 re-observed.
- [ ] **Step 2:** Linux container: HyperCast `cargo no-panic` + canary, HyperTabular `./check-core.sh`
  + canary, both repos' `cargo test`.
- [ ] **Step 3:** Report to the owner: per repo, macOS results, Linux results, what is left to CI
  (Windows, s390x, the Apple mobile legs), every HyperCast backport found (with its HyperCast test),
  and the spec refinements above. Then stop: the owner commits, pushes the HyperCast PR, dispatches
  its `purpose=board` run, and moves HyperTabular's pin with `companion_pin.py` to that exact run and
  commit.
