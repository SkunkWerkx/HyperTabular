# HyperTabular — notes for agents

A Rust core (`rust/`) that reads delimited text and XLSX, ODS and Numbers workbooks into typed
column batches, judging each cell with HyperCast, behind a C ABI reached from nine other
languages: over the C ABI, as a native extension, or, for Go, as Go source translated from its
WebAssembly build, and from C and C++ through the release's C archives. `docs/design.md`,
`docs/delimited.md` and `docs/workbook.md` have the why; all CI and release logic lives in the
Forge (`SkunkWerkx/.github`). This file holds what the code alone won't tell you.

## The contract

- Nothing is sniffed: dialect, plan, number format and date order are declared by the
  caller. A bad cell is a HyperCast verdict (`Empty` / `Malformed` / `OutOfRange` plus a
  span), never an exception: every binding hands out HyperCast's own verdict type in its
  language's idiom. A broken file is a structural error, raised or returned (per binding, as
  `docs/design.md`, "The bindings, side by side", lists) only after every intact row before it
  has been delivered. See `docs/design.md`, "Errors".
- `corpus/delimited.json` and `corpus/workbook.json` (with the workbook packages it names) are
  replayed by every binding, C++ through `c/smoke/corpus.cpp`. `workbook.json` was first
  written by an independent reader that has since been deleted, so a diff to it is a
  behaviour change to explain, not output to regenerate away (`corpus/README.md`). The R
  tarball carries both and every package they name: `r_corpus` in `.github/hyper.json`,
  which a test holds to that list.

## Two layers

- `rust/src/kernel` is the core: `no_std`, never names `alloc`, panic-free, and all memory
  is passed in by the caller. Everything else is "the Rust binding", and exports nothing.
  `rust/check-core.sh` proves this on the built library, with the soname and the static
  library's weak `rust_eh_personality`.
- Every export goes through the `export!` macro in `kernel/exports.rs`. No export takes
  more than twelve integer arguments, because Mono's interpreter (.NET in the browser and
  iOS debug builds) passes no more; extra buffers travel in a struct. `hypertabular_canary`
  is the no-panic proof's negative control and no binding calls it.

## Buffers

A delimited fill that finds a buffer too small consumes nothing and is simply retried. A
workbook call cannot roll back a deflate stream: it stops in place and reports the size it
needs, and the caller must grow that buffer keeping its contents, then repeat the call.
Bindings prove this by replaying the corpus with every buffer starting at one element
(`docs/workbook.md`, "Growing a buffer").

## Bindings differ on purpose

C#, Java, Go, Swift and C++ hand out batch views valid until the next `read()`; Python, Ruby,
PHP, TypeScript and R copy. Ruby fault spans count characters and R's `substr()` units, every
other binding counts bytes (`docs/design.md`, "The bindings, side by side").

## Upstream: HyperCast

HyperCast is the judge, taken with its `exports` feature off so the two libraries link into
one program. Every binding hands out HyperCast's binding types, so a HyperCast release comes
first and a bump touches every manifest: `rust/Cargo.toml` (then `cargo update -p hypercast`
and the lockfiles below), the csproj, `build.gradle.kts`, `pyproject.toml`, the gemspec, the
root `composer.json` (`ext-hypercast`), both `Package.swift` files and
`swift/Benchmarks/Package.resolved`, `go/go.mod` and `go.sum`, `typescript/package.json`
(peer and dev) with its two `bun.lock`s and `typescript/Cargo.lock`, `java/jni/Cargo.lock`,
`r/DESCRIPTION`'s Imports, and the C++ header's minimum. After the core's HyperCast moves,
`go generate` in `go/` and the header test re-run. Dependabot ignores HyperCast everywhere.

HyperCast is HyperTabular's companion. The board (every pull request, the weekly schedule and a
`purpose=board` dispatch) never takes it from a registry: `.github/hyper.json`'s `companions`
block pins one HyperCast commit and the green full-board run of its `ci.yml` that proved it.
`ci.yml`'s `companions` job (the Forge's `hyper-companions.yml`, never gated, the one job that
holds the companion App's key) mirrors that run's artifacts into this run, and every leg's
`fetch-companions`, the Forge's and this repo's four own jobs' (`test-apple-mobile`,
`test-android`, `test-nuget-package`, `test-npm-package`), wires them into the job's tree only
and proves where each install came from. `codeql.yml`'s Swift leg takes the same commit from source for
the core it builds, on every run, so a scan never waits on a registry. A HyperCast artifact renamed in its `ci.yml` is a Forge
`companion_map.py` change. The manifests keep requiring what the code needs, one version across
all of them (the wiring sets each to the pin's exact version in the job's tree). A release takes
HyperCast from the registries instead, so the order is: HyperCast releases, live everywhere but
CRAN, with the Forge attaching its CRAN tarball to its GitHub Release; then this repo's
`prepare-release.yml`, whose `companions` job gates on exactly that
(`python3 ../HyperForge/.github/scripts/companion_gate.py --root .` checks it beforehand); CRAN
last, HyperCast's then this. `.github/RELEASING.md` and the Forge's runbook have moving the pin.
The `companions` job takes the Forge as the run was queued, the four own jobs check it out at
`master` when they start: a Forge merge that changes the mirror's layout while a board run is in
flight can fail their fetch, so don't merge one then, and re-run the whole board if it happened.

## Lists that move together

- Exports: `rust/src/kernel/exports.rs` (cbindgen generates `include/hypertabular.h` from it),
  C#'s three `LibraryImport` declarations (`"hypertabular"`, `"*"`, `"__Internal"`), Java's
  FFM `Native` (`src/main/java25/`), the JNI base `Native`'s natives, `java/jni/src/lib.rs`
  (its extern block and its `RegisterNatives` table) and its `examples/no_panic.rs` (every JNI
  entry point carries `no_panic`), Go's two (`go/internal/core/core.go`, which `go generate`
  rewrites, and `go/core.go`, every export called), the C++ header `include/hypertabular.hpp`
  (hand-written; its `::`-qualified calls are a site), R's C shim `r/src/hypertabular.c`, the
  C smoke consumer `c/smoke/main.c`, Ruby's Fiddle table (`ruby/lib/hypertabular/runtime.rb`),
  Swift's sources, the prefix in `.github/hyper.json` (the Forge's symbol check reads it), the
  export list in the README and `docs/`. `.github/scripts/check_exports.py` fails on drift,
  and holds every `#[napi]` function in `typescript/src/lib.rs` to `catch_unwind` and every
  PHP-callable body in `rust/src/php_ext/` to `guard(` (neither is a C ABI site: both call the
  Rust API), and every extern block there naming the engine's Windows library to the
  `php8`/`php8ts` pair `build.rs`'s `php_zts` picks (`php_link`). The Blazor archive needs no export list: the SDK's pinvoke table links every
  P/Invoke by symbol. Every ABI struct's layout is pinned by `offset_of!` asserts in the
  kernel and carried by the header as `*_SIZE`; the bindings restate them.
- The companion pin: the `companions` block in `.github/hyper.json` (`repo`, `commit`, `run`,
  moved only by the Forge's `companion_pin.py`, never by hand), `ci.yml`'s `companions` job and
  its place in every board job's `needs` (`pr_board.py`'s `JOBS` and the contract tests hold
  both), the fetch and origin steps of the four own jobs above, whose legs are the Forge's
  `companion_map.py` `ci.yml` entries (each mirrors that job's download steps and names every
  binding the job builds against HyperCast, its Go and Swift suites included, so a changed
  download or suite there is a Forge change; `test-android` fetches in two passes, Swift's after
  setup-swift), and `.companions/` in `.gitignore`. A pinned run's
  artifacts expire after 90 days; the `companions` job then names the `purpose=board` dispatch
  to make and pin.
- The shared library's own name is set in three places that must agree: `rust/build.rs`
  (install name `@rpath/libhypertabular.dylib`, soname `libhypertabular.so`; `cdylib` feature
  only), `IMPORTED_SONAME` in `c/cmake/hypertabularConfig.cmake`, and the `needed` check in
  `c/smoke/run.sh`.
- The C archives: `C_RIDS` in the Forge's `c_package.py`, the platform list in `c/README.md`
  and the C row of the README's platform table.
- Ruby ABIs: `ABIS` in `ruby/Rakefile`, `ruby_compat_version` in `ci.yml`, and in the Forge
  the `ruby.abis` default in `hyper_config.py` (an `.github/hyper.json` override if this repo
  ever differs) and the `ruby_wasm_abis` default. Fiddle covers every Ruby and platform outside
  them and must never be restricted. Ruby 3.4 reaches EOL on 2028-03-31: then drop it from
  those lists and clear `ruby_compat_version`.
- npm's ten packages: the root `@skunkwerkx/hypertabular`, eight
  `@skunkwerkx/hypertabular-<suffix>`, one per RID, mapped by `NODE_PLATFORMS` in the Forge's
  `hyper_config.py` and listed as `napi.targets` in `typescript/package.json`, and the
  WebAssembly fallback `@skunkwerkx/hypertabular-wasm32-wasip1` (the last `napi.targets`
  entry). The root's `optionalDependencies` are injected at pack time, never committed;
  `@skunkwerkx/hypercast` is a peer. The first publish of every package is the owner's, by
  hand with a granular token: npm Trusted Publishing attaches only to a package that exists,
  and `release.yml` is registered as its trusted publisher after.
- The Node floor: `engines.node` (`^22.14.0 || >=23.6.0`, Node-API 10's floor; the WASI runtime's is lower) in
  `typescript/package.json`, the same string in the Forge's `npm_package.py`, its
  `node_floor_versions` default, `FLOOR_IMAGE`'s default `node:22.14.0-alpine` and
  `LOCKFILE_NPM` in `typescript/consumer-tests/run.sh`, and the floor sentences in
  `typescript/README.md`, `README.md` and the CHANGELOG. Node 22 reaches EOL on 2027-04-30:
  then drop it from all of them.
- The JDK floor: `jdkFloor` (`21`) in `java/build.gradle.kts`, `jdk_floor_version` in
  `ci.yml`, the HyperCast jar (1.1.0 or later, the first multi-release one), and the floor
  sentences in `java/README.md` and the README. JDK 21 reaches EOL at Temurin on 2029-12-31:
  then the floor is 25, and `java/jni/`, `java_jni`, the JNI sites in
  `check_exports.py` and `checkFloorApi` go, the compile goes back to `--release`, and the FFM
  `Native` moves back into the base.
- The .NET floor: `TargetFrameworks` in `csharp/Directory.Build.props` (the Apple and Android
  smoke tests, the gate probe and the benchmarks set their own), the csproj's HyperCast
  reference (a HyperCast with `lib/net10.0/`, 1.1.0 or later), ci.yml's
  `csharp_aot_frameworks` and `csharp_extra_runtime`, the `HYPERTABULAR003` gate and the
  `build/net10.0/` folder name (which the smoke tests and
  `csharp/HyperTabular.PlatformGateTest` import by path), `test-nuget-package`'s layout step
  (its `lib/` and `build/` paths and the no-`build/net11.0` check), its AOT loop (`net10.0
  net11.0`) and control (`-f net10.0`), the needles `test_workflow_contracts.py` pins for
  that job, its floor step (a net9.0 restore must fail with NU1202) and its gate step, the
  Apple and Android probe steps that hard-code `net10.0-*`, the WasmSmokeTest's per-framework
  references and `check.sh`'s net10 legs and header, `wasm-tools-net10` in the
  `HYPERTABULAR002` text and both READMEs, the Forge `test-blazor` job's .NET 10 SDK and
  `wasm-tools-net10` steps, and the floor sentences in `csharp/README.md` and `README.md`.
  .NET 10 reaches EOL on 2028-11-14: then drop it from all of them, and the targets folder
  moves to the new lowest TFM.
- The PHP floor: `php` in `composer.json` and `php/composer.json` (whose `config.platform.php`
  pins the lock to the floor), `php_floor_version` in `ci.yml`, the lowest of the PHP minors
  below, and the floor sentences in `php/README.md` and the root README. PHP 8.3 reaches EOL
  on 2027-12-31: then drop it from all of them.
- The PHP minors the extension is built for (8.3, 8.4, 8.5): the Forge's `php_versions`
  default, `MINORS`, `WINDOWS_COMPILERS` and `WINDOWS_TOOLSETS` in its `php_ext_package.py`,
  the `minors` default in `hyper-pie-acceptance.yml`, and the platform and minor table in
  `php/README.md`. A new minor is 12 more zips on the release (six platforms, NTS and ZTS); the
  count (36 today) is in `php/README.md` and the CHANGELOG. No Intel macOS zip.
- The R wrapper's core pin: `hypertabular = { version = "=X.Y.Z"` in `r/src/rust/Cargo.toml`
  is a version file the Forge bumps with `r/DESCRIPTION`; `r/tools/vendor.sh` refuses a pin
  that differs from `rust/Cargo.toml`. `r/inst/AUTHORS` is regenerated by `vendor.sh`; never
  edit it by hand.
- The Python sdist: built in the Forge's `hyper-build-wheels.yml` because `.github/hyper.json`
  says `sdist: true`; one more pypi inventory file beside the wheels. Its Rust floor is
  `rust-version`, proven at exactly that toolchain by the build job.
- The Go core's regeneration pins: the rustc in `go/internal/wasm2go/rust-toolchain.toml` (a
  reproducibility constraint, not a floor) and wasm2go's version in its `go.mod`, each moved
  by regenerating and committing the core, and kept equal to HyperCast's. Go's floor is `go`
  in `go/go.mod` (`1.26`) and the floor sentence in `go/README.md`.
- The corpus's keys: every replay refuses a key it doesn't know (`corpus/README.md`), so a new
  key in `corpus/delimited.json` or `corpus/workbook.json` is read in `rust/tests/support/corpus.rs`
  and `corpus_delimited.rs`, `CorpusKeys.cs`, `CorpusKeys.java`, `go/corpus_test.go` and
  `go/workbook_test.go`,
  `CorpusKeys.swift`, `python/tests/test_corpus.py`, `ruby/spec/corpus_helpers.rb`,
  `php/tests/CorpusTest.php`, `typescript/tests/corpus.test.ts`, `r/tests/testthat/test-corpus.R`
  and `c/smoke/corpus.cpp`, and a new package is added to `r_corpus`.
- The core's codes that bindings name: `Failure` (`kernel/abi.rs`, `ENCODING` is 27), a
  `CellNote`'s origins, and `hypertabular_suggest`'s doors, params and statuses (every door,
  `DateTime` included, `NOTATION_COMMA_GROUPS` and `NOTATION_COMMA_DECIMAL`, `NOTATION_FLAGS` 95):
  each binding's enums and its suggest's door table, and the C++ header's.
- The core's size constants in `kernel/size.rs`: `MAX_ROW_BYTES` (1 GiB) is restated by C#,
  Java, Swift, Go, Ruby (`delimited_reader.rb`), R's C shim and the C++ header (`max_row_bytes`), and reaches PHP and Python through
  the Rust API; `CELL_BUDGET` (4096 × 257) by C#, Java, Swift, Go, Ruby (`runtime/delimited.rb`),
  R's C shim and the C++ header. `BATCH_BYTES_LIMIT` (4 GiB) and `SAMPLE_BATCH_BYTES` (64 MiB)
  are the core's alone, asked through `hypertabular_batch_check`; no binding holds a size rule.

## Never hand-edit

- `include/hypertabular.h` and the Swift bundle's copy are generated by cbindgen from
  `rust/src/kernel/exports.rs` and the types its exports take: `HYPERTABULAR_BLESS=1 cargo
  test --test header` in `rust/` rewrites both, and the test fails when either differs. The
  test runs its own `cargo metadata`, so a HyperCast patched in with `--config` does not
  reach it; patch through a `CARGO_HOME` whose `config.toml` holds the `[patch]`, and put
  `rust/Cargo.lock`'s hypercast entry back after.
- `go/internal/core/core.go` is wasm2go's translation of the core's `wasm32-wasip1` build:
  `go generate` in `go/` rewrites it with the rustc pinned in
  `go/internal/wasm2go/rust-toolchain.toml`, and `HYPERTABULAR_CHECK_CORE=1 go test -count=1
  -run 'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .` diffs it. Any change under
  `rust/src` that reaches the cdylib means regenerating it; `prepare-release.yml` regenerates
  it at the version (the version is a literal in it).
- `typescript/binding.js` and `binding.d.ts` are napi-rs output: `bun run build` in
  `typescript/` rewrites them.
- `php/stubs/hypertabular.stub.php` is generated from the extension by `php
  tools/generate-stubs.php` in `php/` (Linux or macOS), with cargo-php pinned to the
  `CARGO_PHP_VERSION` that script names (0.2.2); `StubTest` regenerates and compares it when
  `HYPERTABULAR_CHECK_STUBS=1`.
- The root `Package.swift`'s release version and the two Swift zips' checksums are written by
  `ci.yml`'s `stage-release` job in the preparation run. No binary is committed anywhere.
- Versions: bump through `prepare-release.yml`. The libraries embed their version.

## Claims are measured

Benchmark tables are dated, machine-specific receipts; losses are printed next to wins, and
a row that predates a change says so. Go's translated core is 1.7–2.3x slower than the cgo
binding it replaced (`go/README.md`): that loss stays printed. Don't adjust a number without
re-running its harness, and don't drop a loss.

## Toolchains and floors

Each language's primary toolchain is its current release; check endoflife.date rather than
assuming. Exceptions must be commented where pinned: a real platform constraint, or an
older floor shipped beside the primary and dropped at its EOL. Floors match HyperUuid and
HyperCast and move together.

Rust has two exceptions, both on stable with `RUSTC_BOOTSTRAP=1`, which the Forge sets on
exactly two build steps and nowhere else; never set it more widely. Neither has a stability
promise, so every stable bump is re-proven by the legs that use it.

- The Blazor archive (`hyper-build-wasm.yml`) is built with `-Zbuild-std`, because only that
  leaves out the prebuilt sysroot's legacy exception handling and the features .NET 10's
  wasm-opt rejects (`rust/.cargo/config.toml` says why). When build-std stabilizes, the
  variable goes from that step.
- The Windows PHP extension (`hyper-build-php-extension.yml`'s Windows build step): ext-php-rs
  needs the unstable `abi_vectorcall` there (rust-lang/rust#124485, the `cfg_attr` at the top
  of `rust/src/lib.rs`). When the ABI stabilizes, the variable goes from that step.

## Forge coupling and releases

- The Forge is called at master, deliberately unpinned: a Forge merge reaches every consumer
  at once, and breakage it causes is fixed forward. Each run resolves master to one commit
  (`ci.yml`'s `forge` job, `release.yml`'s `find-ci-run`), checks the Forge out at it as
  `.hyperforge` and runs its composite actions from there. The inventory records the commit
  that packed, and the tag run requires its own Forge to descend from it. Don't merge Forge
  changes while a preparation is in flight.
- The board is one Forge workflow per concern: `core` (`hyper-core.yml`, the seven core legs,
  each with its cdylib export check and C smoke), `core-proofs` (`hyper-core-proofs.yml`, the
  no_std, no-allocator and no-panic proofs, on every pull request that builds the core), `core-checks` (`hyper-core-checks.yml`, the
  static checks on three hosts, s390x included, running `rust/check-core.sh` with the async
  feature beside the default and the Forge's own no-panic job off, since check-core.sh proves
  it with its canary) and `test-<binding>` (one job per binding: its toolchain once, on a pull request that touched
  its tree or `.github/` its linters on one leg, then its suite on its hosts), gated on what the pull request touched (`changes`, from
  `.github/scripts/pr_changes.py`; `pr-board` is the one status to read, red when a job failed
  or was skipped while its gate said run). Off a pull request nothing is gated and the whole
  release-mode board runs. A new binding is one `test-` caller, one class in
  `pr_changes.py` and one gate in `pr_board.py` (its tests hold the three together).
- `prepare-release.yml` and ci.yml's `core-checks` are one `uses:` each of a Forge reusable
  workflow; this repo keeps triggers, permissions and gates. `.github/hyper.json` is what the
  Forge knows about this project (crate and project names, the export rule with the canary
  excluded, the sdist, `swift_remote` and `c_assets` switches, `r_corpus`, and
  `big_endian_control`, the corpus vector `core-checks` corrupts on s390x to prove the suite
  reads the corpus there and the test that must then fail; the ABI probe is
  `.github/scripts/abi_probe.c`, the Forge's default path; every other name is derived in the
  Forge's `hyper_config.py`). `.github/scripts/tests` holds the workflow shape contracts and
  the probe, export, board and text-file tests; the generic release-guard tests live in the
  Forge.
- The release is two acts: `prepare-release.yml` pushes A, and that run's release-mode
  `ci.yml` packs every registry input, attests each package and the inventory at A, preserves
  `release-packages` for 90 days, runs CodeQL on A and commits B; the tag push at B verifies
  the preserved bytes, signs the release attestation from the tag and publishes. `gh workflow
  run release.yml --ref vX.Y.Z -f publication_run_id=<tag run>` replays it with the receipts
  (every release runs that once, as its no-op recovery proof). The finalizer writes the GitHub
  Release from the CHANGELOG's `## [X.Y.Z]` section; `prepare-release.yml` dates that heading,
  sets `r/NEWS.md`'s, and refuses a CHANGELOG without it or with a placeholder left in it.
- Third-party actions are pinned to a commit with the version in a trailing comment, and
  `.github/dependabot.yml` moves them with every other ecosystem in one monthly pull request.
  It ignores the Forge and `dtolnay/rust-toolchain` (branch heads, refreshed by hand), and
  HyperCast in every ecosystem.
- This repo conforms to the Forge; it doesn't add opt-outs (`ci.yml` header).
- Registry versions can never be replaced. Release and recovery: `.github/RELEASING.md`
  and the Forge's `docs/release-runbook.md`. Recover by re-dispatching from the preserved
  packages, never by retagging or rebuilding. CRAN submission is by hand, from the R tarball
  the tag run preserved, after HyperCast's own.

## Local loop

The PR board always builds the core and proves its thesis (`core-proofs`). A binding's suite runs when its tree, the core (`rust/`,
`include/`, `c/`, `corpus/`) or `.github/` changed, with its linters ahead of it when its tree or `.github/` did;
Rust's lint, in `core`, when the core or `.github/` did; `python3 .github/scripts/pr_changes.py`
classifies a `filename<TAB>previous_filename` list from stdin the way the `changes` job does
(`--all` is the release board), and `python3 .github/scripts/pr_board.py` with `NEEDS`,
`CHANGES` and `GITHUB_EVENT_NAME` set gives the verdict.

Until HyperCast's next version is on its registries, every binding verifies against
`../HyperCast`'s working tree through overrides that stay out of the tree: a cargo
`[patch.crates-io]`, a `go.work` with a `replace`, a local NuGet feed, Maven repository, npm
tarballs, pip and R installs. The pin's own artifacts do the same for every binding at once: a board
run's `companion-hypercast` artifact (`gh run download`), then the Forge's `companion_fetch.py`
`stage`, `env` and `wire` per leg, in a scratch clone and never the main checkout (it rewrites
manifests and lockfiles). That wiring writes where a runner's home is: set `CARGO_HOME`,
`GRADLE_USER_HOME`, `R_LIBS_USER`, `GITHUB_ENV` and `HYPER_COMPANION_PYTHON` (a venv) to scratch
first, or it patches `~/.cargo/config.toml` and `~/.gradle/init.d`.

`cd rust && cargo test`, then `./check-core.sh`, which is Linux-only (`.so`, `nm -D`,
`readelf`): on a Mac run it in Docker (`rust:latest`, the three `cargo check` targets and
`llvm-tools` added). `cargo no-panic` passes vacuously on Darwin, because `.cargo/config.toml`
links Darwin targets with `-undefined,dynamic_lookup`; the honest Mac proof is the same `cargo
rustc` with `RUSTFLAGS=""` (the env overrides the config) into its own `--target-dir`, and its
negative control is `RUSTFLAGS="--cfg hypertabular_canary"` failing on `hypertabular_canary`
alone. Build `cargo cdylib` before a binding's tests. A raw `cargo rustc --release
--crate-type cdylib --features python` (or `ruby`, or `php`) in `rust/` overwrites the same
`target/release` library; maturin in `python/` and the `ruby-ext`/`php-ext` aliases build
elsewhere and don't.

The suite also runs under wasmtime: `cargo test --target wasm32-wasip1 --target-dir <target>`
with `CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime run --dir=<repo>::<repo>
--dir=<target>::<target>"`, both mounts because the corpus is read by absolute path and
`CARGO_TARGET_TMPDIR` is in the target dir. libtest skips the `should_panic` tests there.
Private core tests stay in `rust/tests/`, included by `lib.rs` through `tests/internal.rs`.
Cargo lists integration targets explicitly; register a new integration test in
`rust/Cargo.toml`. Allocation-counting tests remain separate integration binaries. Large
benchmark inputs come from the ignored `corpus/generate/out/`.

Swift has no core until one is built: `.github/scripts/local-core.sh` builds the checkout's own
into the gitignored `swift/HyperTabularCore.artifactbundle/` (it finds `../HyperForge` on its
own; `FORGE=<path>` otherwise) and stages the same archive in
`csharp/HyperTabular/staticlibs/<rid>/` for a local Native AOT publish, then `swift test`. It
starts with `.github/scripts/local-clean.sh`, which removes every binding's staged build
artifact and names what rebuilds each; run it alone before any local build after a `rust/`
change. CI builds each leg's core into an empty tree.

Go needs nothing built: `go test ./...` in `go/` runs on the committed translated core
(`CGO_ENABLED=0`, as CI runs it). After a change in `rust/`, `go generate` in `go/`, then the
regeneration check above (`-count=1`, since `go test` replays a cached pass). The wasm legs:
`tinygo test -target=wasip1` with the corpus mounted, and `GOOS=js` or `GOOS=wasip1` with
`GOARCH=wasm` `go test ./...` with `$(go env GOROOT)/lib/wasm` on PATH.

PHP is the extension built from `rust/`: `cargo php-ext` there, then `composer test` in `php/`
with ext-hypercast loaded beside it (`HYPERCAST_PHP_EXTENSION` names HyperCast's build,
`HYPERTABULAR_PHP_EXTENSION` this one; `composer bench` for the benchmarks);
`HYPERTABULAR_CHECK_STUBS=1 composer test` runs the stub check (cargo-php and a C compiler
needed).

Java's JNI path needs its own library: `cargo build --release --locked --lib` in `java/jni/`,
after `cargo cdylib`. Gradle stages both and refuses a stale one. Then, from `java/`,
`./gradlew test testFloor checkSuiteParity javadoc` with a JDK 21 passed through
`-Dorg.gradle.java.installations.paths=<jdk21>`.

TypeScript builds its own addon, a napi-rs crate path-depending on `rust/`: from
`typescript/`, `bun install --frozen-lockfile && bun run build && bun run test`, then `bun run test:bun` and `bun run
lint`. The WebAssembly fallback (`rustup target add wasm32-wasip1`): `bun run build:wasi`, then
`NAPI_RS_FORCE_WASI=error bun run test`. The consumer tests are `typescript/consumer-tests/README.md`
(Verdaccio on port 4873; `floor` and `s390x` need Docker).

R builds the core from its vendor tarball, never from `rust/` directly: `r/tools/vendor.sh`
resolves the pinned version from the checkout's `rust/`, vendors the core and HyperCast into
`r/src/rust/vendor.tar.xz` (`HYPERCAST_PATH=../HyperCast/rust` vendors an unpublished
HyperCast), derives the wrapper's lock, stages the header and the corpus and rewrites
`inst/AUTHORS`, and has to run again after any change in `rust/`, `include/` or `corpus/`. Then,
from the repo root, `Rscript -e 'devtools::test("r")'` and `R CMD check --as-cran` on the
tarball `R CMD build r` makes.

C builds an archive from local libraries with the Forge's packer, then the smoke consumers
against it (CMake and pkg-config on PATH), after `cargo cdylib` and `local-core.sh`:

```sh
python3 ../HyperForge/.github/scripts/c_package.py assemble --rid osx-arm64 --out <dir> \
  --static swift/HyperTabularCore.artifactbundle/arm64-apple-macosx/libhypertabular.a \
  --shared rust/target/release/libhypertabular.dylib
SMOKE_CXX=1 c/smoke/run.sh <dir>/hypertabular-<version>-osx-arm64 <version>
```

The C++ smoke takes HyperCast's C archive from `HYPERCAST_PACKAGES=<dir>`, or downloads it from
HyperCast's GitHub Release at the version `rust/Cargo.toml` requires. `SMOKE_ARCH=x86_64` runs
an osx-x64 archive under Rosetta, and `c/smoke/apple-mobile.sh <dir> <version>` takes the four
Apple mobile archives.

The Blazor archive needs stable's sources: `rustup component add rust-src` and `rustup target
add wasm32-unknown-emscripten`, emsdk on PATH, then `RUSTC_BOOTSTRAP=1 cargo wasm-staticlib`
in `rust/`, after deleting `rust/target/.rustc_info.json` and
`rust/target/wasm32-unknown-emscripten/` (cargo caches rustc's cfg without regard to the
variable); the Forge's `wasm_archive_guard.py` checks the result.

## Keeping the owner's Mac alive

Runs here have taken the Mac down: build artifacts filled its disk (before `local-core.sh` began
with `local-clean.sh`), a size probe filled its RAM (2026-11-07, 959 processes killed,
force-quit), HyperCast's `swift test` filled the process table with about 2,600
`swiftpm-testing-helper` copies (2026-11-08, nothing could fork, reboot), and agents' wait loops
never ended. Every session and every subagent follows these rules; a brief repeats them.

- Disk: every `--target-dir`, staged library and Docker image costs gigabytes (one gate run's
  scratch reached 26 GiB). Reuse a target dir rather than make a new one, check `df -h ~`
  before a run that makes several, and delete your own scratch target dirs as soon as their
  results are recorded. In the main checkout, `local-clean.sh` clears the staged artifacts, alone
  and only when no other agent is building against them.
- Memory: never run a size probe, or anything that may allocate without bound, on the host.
  `timeout` doesn't help and macOS ignores `ulimit -v`: run it in Docker with `--memory=4g
  --memory-swap=4g` and a `timeout`, or reason from the source. A refusal test refuses before
  touching memory and joins a suite only after Docker proved it; a test needing gigabytes skips
  itself out loud and states its measured footprint.
- Processes: run every `swift test`, and anything else that spawns test children (Swift Testing
  exit tests, fork-per-test runners), in a subshell capped from this user's own process count,
  `( ulimit -u $(( $(ps -U $(id -u) | wc -l) + 80 )); timeout 300 swift test ... )`, and check
  that no helper is left after. Size it from `ps -U`, never `ps -ax` (that left ~530 spare slots).
  The 2026-11-08 swarm's cause is unknown (intermittent, seen only from subagent shells);
  HyperCast's `StylesRefusalExitTests` is the only suite that spawns the helper.
- One heavy job at a time: any build, suite, benchmark, Docker run, mutant or differential storm.
  When agents run in parallel, each heavy job takes a lock in one command, `until mkdir
  ~/.cache/hyper-gate/heavy.lock 2>/dev/null; do sleep 20; done; trap 'rmdir
  ~/.cache/hyper-gate/heavy.lock' EXIT; <job>`, never held across thinking. A lock older than 30
  minutes with nothing heavy running is stale.
- A long unattended run keeps a guard alive: `~/.cache/hyper-gate/memguard.py` kills any process
  above 16 GiB, the largest when swap passes 8 GiB or memory pressure goes critical, and any
  command with more than 300 copies, and acts on its last process table when it can't fork. A
  `KILL` line in its log is a finding: report it, never retry at the same size or without the cap.
- Waiting: never `while pgrep -f <name>` (the loop matches its own command line and never ends)
  and no foreground wait that can outlive its timeout; background the job and take its
  notification, or wait on its PID.
- Never print a process environment (`ps -E`, `env`, `/proc/*/environ`): the owner's shell holds
  tokens.
- Never run `local-core.sh` as it is inside an unattended run: it removes every binding's staged
  artifacts. Build the core step by step as it does.
- Scratch lives under `~/.cache/<run>/`, never `/private/tmp` (a reboot wipes it), and goes when
  done. Never delete shared caches or prune Docker to make room.

## Bun and npm

Bun is the package manager and script runner for every JavaScript tree here, locked by `bun.lock`.
npm is confined to the published packages: the Forge's `npm pkg set` and `npm pack`, `npm publish`,
and the consumer scenarios that install as an npm user does (`bun-install` is the Bun user's; Bun
has no s390x build, so the `s390x` scenario could not move in any case). Don't add another npm call.
Bun's own runtime still can't load the WASI module, so `test:bun` under `NAPI_RS_FORCE_WASI=error`
stays recorded, never gating. `bun run test` is the package script; `bun test` is Bun's runner.

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
- A change reaches every binding (C++'s header included) and the sibling repos in the same
  pass, with parity in behaviour, API and wording. Where a binding can't follow, say why for
  that binding. Never leave a repo half-edited.
- Pre-1.0, a breaking change is a plain CHANGELOG entry under Unreleased; no semver
  callout, no upgrade hand-wringing.
- Install snippets keep `<version>` (Go: `@latest`); a concrete version there is the bug.
- Drafting release notes includes the pre-publish fixes: rename `[Unreleased]`, add the
  bullet for every package the diff touched, and set `r/NEWS.md`'s heading to the version,
  then draft from the fixed changelog.
- Never report commit, push or release state from memory; check with git or gh in the same
  turn, or leave it out.
- A verification counts only when its negative control fails. Copy the flags the repo's own
  alias or CI step uses before trusting a check.
- Formatters and their CI checks cover src, tests, benchmarks and smoke tests in every
  language.
- Tags are annotated and signed: `git tag -m "HyperTabular vX.Y.Z" vX.Y.Z <B>` (`tag.gpgsign` is on,
  so the tag carries the owner's SSH signature); commits are always signed.
- Remove agent worktrees and their branches once the work is in the main checkout;
  `.claude/` is untracked and `git add` picks it up.

## Open follow-ups (as of 2026-11-08)

- The next release waits on HyperCast 1.1.0 on every registry. HyperCast 1.1's work is on
  HyperCast's `master` (`f558c7e`, still versioned 1.0.0), and the companion pin in
  `.github/hyper.json` names that commit and its green run. Every manifest requires 1.1.0, but the committed lockfiles
  (`rust/`, `rust/browser-test/`, `rust/fuzz/`, `java/jni/`) and
  `swift/Benchmarks/Package.resolved` still name 1.0.0, which only the bump above can move
  once 1.1.0 is published; the board meanwhile runs on the pin, whose wiring relocks each job's
  tree. HyperCast and HyperTabular 1.1 go to CRAN together, HyperCast's first.
- The next release is the first with npm, CRAN, C archives, PIE zips, the Swift zips and the
  JNI jar; what only it can prove is the same list as HyperCast's.
- Benchmark tables: the workbook table through every binding and Go's and PHP's receipts were
  measured on an Apple M6 on 2026-11-08, one session; the delimited and workbook core against 1.0
  (tag `v1.0.0` over HyperCast 1.0.0, with a third build splitting each loss between the two
  libraries) on 2026-11-09, after the sink, bad-row and shared-string fixes; the tables through
  the bindings predate those three. The WSL2 i9-11900H receipts left (pre-1.0, in `docs/delimited.md`, `docs/workbook.md`
  and `docs/design.md`) say so. Re-measure every harness on one machine before relabelling a
  table; never mix machines.
- Ruby's Fiddle path reads an xlsx sheet 54% and an ods one 30% slower than the Magnus
  extension (`ruby/README.md`, 2026-11-08). A three-column sheet read level on 2026-11-07, before
  the 2026-11-08 changes to Ruby's read path; the cause isn't known.
- One long record from a stream whose reads come back short costs the square of its length
  (256 MiB at 64 KiB a read, about 24 s in Rust, the same shape in Java and Swift): the core
  scans an unfinished record again from its start each fill. Parked, with numbers, in
  `docs/delimited.md`.
- Owner decisions held for later: a sheet row of only uncached formulas is filtered by
  `skip_empty_rows` (kept for this release, `skip_empty_rows=false` documented as the lossless
  route); a workbook's date columns suggest text (native date inference parked).
- Java does not reach Android: the JNI base still works in `java.lang.foreign`'s memory API
  (`Arena`, `MemorySegment`), which ART lacks, so Android would need a heap or `ByteBuffer`
  data plane. Python, Ruby, PHP and R don't reach Android either.
- Performance candidates for the pass after the CRAN gate, with their receipts and how to
  measure them: `docs/performance-candidates.md` (gathered 2026-11-09 from the Task 23c report,
  the docs' "Parked" sections and these follow-ups).
