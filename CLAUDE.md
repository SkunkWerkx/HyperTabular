# HyperTabular — notes for agents

A Rust core (`rust/`) that reads delimited text and XLSX/ODS workbooks into typed column
batches, judging each cell with HyperCast, behind a C ABI called from seven other
languages. `docs/design.md`, `docs/delimited.md` and `docs/workbook.md` have the why; all
CI and release logic lives in the Forge (`SkunkWerkx/.github`). This file holds what the
code alone won't tell you.

## The contract

- Nothing is sniffed: dialect, plan, number format and date order are declared by the
  caller. A bad cell is a verdict (`Empty` / `Malformed` / `OutOfRange` plus a span), not
  an error. A broken file is a structural error, raised only after every intact row before
  it has been delivered. See `docs/design.md`, "Errors".
- `corpus/delimited.json` and `corpus/workbook.json` are replayed by every binding.
  `workbook.json` was first written by an independent reader that has since been deleted,
  so a diff to it is a behaviour change to explain, not output to regenerate away
  (`corpus/README.md`).

## Two layers

- `rust/src/kernel` is the core: `no_std`, never names `alloc`, panic-free, and all memory
  is passed in by the caller. Everything else is "the Rust binding", one of eight, and
  exports nothing. `rust/check-core.sh` proves this on the built library.
- Every export goes through the `export!` macro in `kernel/exports.rs`. No export takes
  more than twelve integer arguments, because Mono's interpreter (.NET in the browser and
  iOS debug builds) passes no more; extra buffers travel in a struct.

## Buffers

A delimited fill that finds a buffer too small consumes nothing and is simply retried. A
workbook call cannot roll back a deflate stream: it stops in place and reports the size it
needs, and the caller must grow that buffer keeping its contents, then repeat the call.
Bindings prove this by replaying the corpus with every buffer starting at one element
(`docs/workbook.md`, "Growing a buffer").

## Bindings differ on purpose

C#, Java, Go and Swift hand out batch views valid until the next `read()`; Python, Ruby and
PHP copy. Ruby fault spans count characters, every other binding counts bytes
(`docs/design.md`, "The bindings, side by side").

## Upstream: HyperCast

HyperCast is the judge, taken with its `exports` feature off so the two libraries link
into one program. A HyperCast bump touches every manifest: `rust/Cargo.toml`, the csproj,
`build.gradle.kts`, `pyproject.toml`, the gemspec, `composer.json`, both `Package.swift`
files and `go.mod`.

## Lists that move together

- The export list appears in the README, `docs/design.md`, `docs/delimited.md`,
  `docs/workbook.md`, the binding READMEs and every binding's declarations; the Forge reads it
  from `rust/src/kernel/exports.rs` by the rule in `.github/hyper.json` (the prefix there is
  what its symbol check recognises). The Blazor archive needs no export list: the SDK's
  pinvoke table links every P/Invoke by symbol.
- Ruby ABIs: `ABIS` in `ruby/Rakefile`, `ruby_compat_version` in `ci.yml`, and in the Forge
  the `ruby.abis` default in `hyper_config.py` (an `.github/hyper.json` override if this repo
  ever differs) and the `ruby_wasm_abis` default. Fiddle
  covers every Ruby and platform outside them and must never be restricted. Ruby 3.4
  reaches EOL on 2028-03-31: then drop it from those lists and clear
  `ruby_compat_version`.

## Never hand-edit

- `go/staticlib/`, the Swift artifact bundle and `php/src/native/` are CI-built binaries
  committed by `stage-native-binaries.yml`.
- Versions: bump through `prepare-release.yml`. The libraries embed their version.

## Claims are measured

Benchmark tables are dated receipts. Don't adjust them without re-running every harness.

## Toolchains and floors

Each language's primary toolchain is its current release; check endoflife.date rather than
assuming. Exceptions must be commented where pinned: a real platform constraint, or an
older floor shipped beside the primary and dropped at its EOL. Floors match HyperUuid and
HyperCast and move together.

## Forge coupling and releases

- The Forge is called at master, deliberately unpinned: a Forge merge reaches every consumer
  at once, and breakage it causes is fixed forward. `release.yml` resolves master to one
  commit per run (`find-ci-run`'s `forge_sha`), checks the Forge out at it as `.hyperforge`
  and runs every job body as a composite action from there (`release-guard`, `pack-gems`,
  `verify-wheels`, `release-gate`, `publish-*`, `finalize-release`); the inventory records
  the commit and recovery requires it unchanged. Don't merge Forge changes while a release
  is in flight.
- `prepare-release.yml`, `stage-native-binaries.yml` and ci.yml's `check-core` are one
  `uses:` each of a Forge reusable workflow; this repo keeps triggers, permissions and gates.
  `.github/hyper.json` is what the Forge knows about this project (crate and project names,
  the export rule, the ABI probe; every other name is derived in the Forge's
  `hyper_config.py`). `check-core` runs `rust/check-core.sh` and turns the Forge's own
  no-panic job, `check_exports.py` and SwiftLint off (see its comment). `.github/scripts/tests`
  holds the workflow shape contracts and the probe tests; the generic release-guard tests
  live in the Forge.
- The release is two acts at one tag: the tag push packs, attests (build provenance and the
  release attestation over every shipped file) and preserves without uploading; `gh workflow
  run release.yml --ref vX.Y.Z -f prepared_run_id=<tag run>` publishes from the preserved
  bytes. The finalizer writes the GitHub Release from the CHANGELOG's `## [X.Y.Z]` section,
  and the tag run refuses a CHANGELOG without it.
- Third-party actions are pinned to a commit with the version in a trailing comment, and
  `.github/dependabot.yml` moves them with every other ecosystem in one monthly pull request.
  It ignores the Forge and `dtolnay/rust-toolchain` (branch heads, refreshed by hand), and
  HyperCast in every ecosystem: the upstream is bumped by hand, with the matching binding
  changes.
- Registry versions can never be replaced. Release and recovery: `.github/RELEASING.md`
  and the Forge's `docs/release-runbook.md`.

## Local loop

`cd rust && ./check-core.sh && cargo test`. Bindings need `cargo cdylib` first; each binding
README has its own steps. Large benchmark inputs come from the ignored
`corpus/generate/out/`.

`check-core.sh` is Linux-only (`.so`, `nm -D`, `readelf`): on a Mac run it in Docker
(`rust:latest`, the three `cargo check` targets and `llvm-tools` added). `cargo no-panic`
passes vacuously on Darwin, because `.cargo/config.toml` links Darwin targets with
`-undefined,dynamic_lookup`; the honest Mac proof is the same `cargo rustc` with
`RUSTFLAGS=""` (the env overrides the config) into its own `--target-dir`, and its
negative control is `RUSTFLAGS="--cfg hypertabular_canary"` failing on `hypertabular_canary`
alone.

## Working with the owner

- The owner drives git. Don't commit, push, tag, open pull requests or dispatch workflows
  unless asked for that step by name; a go on the edits is not a go on git. Leave the work
  in the working tree, verified, and report it.
- Agreeing that an approach is sound, or stating a preference, is a design decision, not
  an instruction to start. Reading to scope the work is fine; edits wait for the go. During
  a release the owner may be acting in parallel: `git fetch` and compare with origin before
  editing.
- Prove everything locally first, installing toolchains under `~` as needed. Every push to
  a PR branch runs the full matrix, so only what cannot run locally goes to CI, and the
  report says which parts those were.
- Fix stale docs, comments and links on sight; that permission is standing. READMEs state
  the current state only, history goes in the CHANGELOG, and README examples are verified
  by hand in the same pass. No CI step for docs.
- A change reaches all seven bindings and the sibling repos in the same pass, with parity
  in behaviour, API and wording. Where a binding can't follow, say why for that binding.
  Never leave a repo half-edited.
- Pre-1.0, a breaking change is a plain CHANGELOG entry under Unreleased; no semver
  callout, no upgrade hand-wringing.
- Install snippets keep `<version>` (Go: `@latest`); a concrete version there is the bug.
- Drafting release notes includes the pre-publish fixes: rename `[Unreleased]`, add the
  bullet for every package the diff touched, then draft from the fixed changelog.
- Never report commit, push or release state from memory; check with git or gh in the same
  turn, or leave it out.
- A verification counts only when its negative control fails. Copy the flags the repo's own
  alias or CI step uses before trusting a check.
- Formatters and their CI checks cover src, tests, benchmarks and smoke tests in every
  language.
- Tags are lightweight and unsigned (`git -c tag.gpgsign=false tag`); commits are always
  signed.
- Remove agent worktrees and their branches once the work is in the main checkout;
  `.claude/` is untracked and `git add` picks it up.

## Open follow-ups (as of 2026-10-23)

- Benchmark tables were measured on the WSL2 i9-11900H box. When the dev machine changes,
  re-measure every harness there before relabelling a table; never mix machines, and say
  which tables predate a core change. A benchmarking workflow is an idea, not started.
- 1.0.0 is the first release on the two-step flow. What only it can prove: the composite
  actions in a real job, PyPI Trusted Publishing, nuget.org's finalized bytes, the Maven
  confirmation wait, the finalizer. Afterwards the Forge drops its schema-1 inventory paths.
- `swift/` has no `.swiftlint.yml`, so the Forge's SwiftLint job (`missing_docs`, as HyperCast
  and HyperUuid run it) is off in `ci.yml`; add the config and turn `lint_swift` on.
- Java does not reach Android: the binding uses FFM, which ART lacks, so it would need a
  JNI layer. Python, Ruby and PHP don't reach Android either.
- Workbook performance candidates after the 2026-10-05 tokenizer pass: `docs/workbook.md`,
  "Parked".
