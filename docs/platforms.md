# Platforms

The platform table, the exceptions to it and the runtime floors are in the main README's
[Platform support](../README.md#platform-support). This page has the rest.

## Linking the core in

Two bindings link the core into the consumer's executable where they can, instead of loading a
shared library at run time, and a third carries it as source. Swift always does, on every
platform in the table, so there is nothing to deploy beside the executable: SwiftPM downloads
the core as a binary target from the GitHub Release, checks it against the checksum in the
manifest, and links the archive for the target, its musl cells through Swift's static Linux SDK;
the same mechanism compiles the binding to WebAssembly (see [WebAssembly](#webassembly)). Go
links nothing and loads nothing: its core is Go source, the crate's `wasm32-wasip1` build
translated by [wasm2go](https://github.com/ncruces/wasm2go) and committed as
`go/internal/core/core.go`, so a Go binary carries the core the way it carries any package, on
every GOOS and GOARCH Go has, with `CGO_ENABLED=0` and no C compiler. C# does for a Native AOT
publish, on every RID, so the result is one executable. Python, Ruby's Magnus backend, PHP,
TypeScript and R carry the core inside their own native module (the PyO3, Magnus and ext-php-rs
extensions, the `.node` addon and the R package's compiled library), so none of them loads a
separate core library either. The rest — C# under the JIT, Java (its JNI library on JDK 21–24,
`libhypertabular` through FFM on 25+) and Ruby's Fiddle fallback — load a shared library. C and
C++ take either from the archive: the shared library or the static one.

Every one of them links or loads HyperCast's core beside this one, and the two share no
symbols: `libhypertabular` is built with HyperCast's `exports` feature off, so it carries none
of HyperCast's, and each archive carries a weak hidden `rust_eh_personality`, so two Hyper*
static libraries, or one and a Rust program's own standard library, link as one.

## iOS, Mac Catalyst and Android

**iOS and Mac Catalyst** load no libraries at all, so C# and Swift link the core into the app
there, from static libraries cross-compiled in the same run as the rest: C# for `ios-arm64`,
`iossimulator-arm64`, `maccatalyst-arm64` and `maccatalyst-x64`, where a .NET 11 iOS, MAUI or
Mac Catalyst app (`net11.0-ios`, `net11.0-maccatalyst`) needs nothing but the package reference
(a .NET 10 project for either stops at build with `HYPERTABULAR003`); Swift for the three arm64
ones, as an XCFramework, from iOS 16 and Mac Catalyst 16. The C archives carry the same four
static libraries for a C or C++ app. Go needs none: its core is Go source, so `GOOS=ios` builds
it as it builds any package, and what an iOS executable needs is Xcode's linker, because Go's
`ios/arm64` port links externally, which is Go's rule and nothing of this binding's. CI's
`test-apple-mobile` job, on the release-mode board, builds them on a Mac from that run's
archives: C# and Swift run in an iOS simulator and as a Mac Catalyst process and each links an
iOS device build; Go's suite runs in the simulator and builds for a device; the C smoke
consumer is linked for a device, run in the simulator and run as Mac Catalyst on both
architectures. See [`csharp/README.md`](../csharp/README.md#the-shape),
[`swift/README.md`](../swift/README.md#linking-and-deployment),
[`go/README.md`](../go/README.md#ios-and-mac-catalyst) and [`c/README.md`](../c/README.md#install).

**Android**, arm64 and x86_64, is reached by the same three bindings, so that a .NET MAUI app
has the core on every platform MAUI targets and a Go or Swift Android build has it as on any
other platform. C# carries a shared library cross-built with the NDK at API level 21, its
segments aligned to the 16 KB pages Android 15 devices may use and Google Play requires: an app
on CoreCLR, .NET 11's Android runtime, loads it out of the APK like any Linux library, and a
Native AOT publish links it in from a static archive like every other RID. Go builds for
`GOOS=android` with no NDK: `android/arm64` links internally, and `android/amd64`, which Go
itself links externally, takes the NDK's clang for that reason alone and nothing of the core's.
Swift links its archives from its artifact bundle with the
[Swift SDK for Android](https://www.swift.org/documentation/articles/swift-sdk-for-android-getting-started.html)
(Swift 6.3+, API 28+). The C archives for android-arm64 and android-x64 carry the same
libraries. HyperCast reaches Android the same ways, and its libraries sit beside these in the
same app. CI's `test-android` job builds the C# app both ways for both RIDs from a package
packed in that run, cross-builds the Go and Swift suites, runs everything x86_64 in an emulator
whose image uses 16 KB pages, and links or inspects the arm64 builds. Java, Python, Ruby, PHP,
TypeScript and R do not reach Android. See [`csharp/README.md`](../csharp/README.md#android),
[`go/README.md`](../go/README.md#android) and
[`swift/README.md`](../swift/README.md#linking-and-deployment).

## Publishing

Every binding is published. C#, Java, Ruby, PHP, Python, Rust and TypeScript go through a real
package registry (NuGet, Maven Central, RubyGems, Packagist for PIE, PyPI, crates.io, npm), and
R goes to CRAN as a source package, its tarball attested and preserved by the release like every
other package and submitted to CRAN by hand from those bytes. Go and Swift resolve straight from
a git tag (`go get`, Go's prefixed `go/v<version>`; `.package(url:, from:)`), which is their
whole publish story; Swift's tag names its core as two zips on the GitHub Release,
`HyperTabularCore.artifactbundle.zip` and `HyperTabularCoreApple.xcframework.zip`, by URL and
checksum. PHP's `composer.json` lives at [the repository root](../composer.json) rather than
`php/`, because Packagist requires the manifest at the top of the git repository it watches;
Swift's root [`Package.swift`](../Package.swift) exists for the same reason.

- **Ruby:** nine gems. Seven precompiled platform gems carry the Magnus extension
  (`x86_64-linux-gnu`, `aarch64-linux-gnu`, `x86_64-linux-musl`, `aarch64-linux-musl`,
  `arm64-darwin`, `x64-mingw-ucrt`, `aarch64-mingw-ucrt`), each fat across Ruby 3.4 and 4.0,
  since a Magnus extension is tied to one Ruby minor; a universal gem runs on Fiddle everywhere
  outside that grid (Ruby 3.3, a Ruby newer than the release, Intel macOS, any other platform);
  and `hypertabular-wasm` carries the extension prebuilt for ruby.wasm
  ([Ruby in the browser](../ruby/README.md#ruby-in-the-browser)).
- **Python:** 17 wheels (`cp311-abi3` and `cp315-abi3.abi3t` on each of eight platforms, and
  one for Pyodide) and the sdist, which pip builds where no wheel fits (Rust 1.89 or newer).
- **PHP:** 36 extension zips on the GitHub Release, NTS and ZTS: PHP 8.3, 8.4 and 8.5 on Linux
  x64 and arm64 (glibc and musl), macOS arm64 and Windows x64. PIE reads the package from
  Packagist and downloads the zip for the platform, PHP minor and thread safety
  ([`php/README.md`](../php/README.md#install)).
- **TypeScript:** ten npm packages: the root `@skunkwerkx/hypertabular`, eight platform
  packages `@skunkwerkx/hypertabular-<platform>`, one addon each, which the root names in
  `optionalDependencies` so npm installs the one that fits and nothing compiles, and the
  WebAssembly fallback `@skunkwerkx/hypertabular-wasm32-wasip1`, a plain dependency of the root.
- **C and C++:** fifteen archives on the GitHub Release, one per platform — the eight desktop
  RIDs, android-arm64, android-x64, ios-arm64, iossimulator-arm64, maccatalyst-arm64,
  maccatalyst-x64 and wasi-wasm32 — each with the generated C header, the C++17 header, the
  shared and static libraries (static only for the Apple mobile and WebAssembly ones), and the
  files CMake's `find_package` (or `FetchContent`) and pkg-config read ([C](../c/README.md)).
- **Go** commits no binary: its core is generated Go source in the tree, regenerated and diffed
  in CI. **Java** is plain Java, not Kotlin, so no `kotlin-stdlib` reaches a consumer; its jar
  carries the core and the JNI library for every platform, and its AOT story is proven the way
  C#'s is, by a GraalVM Native Image smoke test (`java/aot-smoke-test/`).

No binary is committed anywhere in the repository.

## WebAssembly

WebAssembly meets a binding in one of two directions, and they share nothing mechanically:

- **The core runs as wasm inside the binding.** The process stays native, and a wasm engine
  the ecosystem already has runs the core as a `wasm32-wasip1` module in-process. One binding
  ships this today, TypeScript's fallback package.
- **The binding is compiled to wasm.** The whole consumer app becomes a wasm module (Blazor, a
  `wasm32` Rust crate, Python under Pyodide) and the core has to be linked into that build by
  the ecosystem's own toolchain.

The core imports nothing (no clock, no entropy, no allocator: the caller's buffers are all the
memory it touches), so a page loads one module and needs no shim beyond what the language
already brings. Where each of the eleven stands, today:

| Binding | Core as wasm inside the binding | Binding compiled to wasm |
| --- | --- | --- |
| Rust | Not applicable: the crate *is* the core. | **Yes.** The full suite passes under [wasmtime](https://wasmtime.dev/) on `wasm32-wasip1` (libtest skips its three `should_panic` tests there, where a panic aborts), run by CI's `core-checks` job whenever the core changes, and `rust/browser-test` runs the crate on `wasm32-unknown-unknown` in headless Chrome. `RUSTC_BOOTSTRAP=1 cargo wasm-staticlib` builds the `wasm32-unknown-emscripten` archive C# links, on stable Rust with `-Zbuild-std` for that one build; the `wasm32-wasip1` archive is what Swift's WebAssembly build links, and the `wasm32-wasip1` module is what wasm2go translates into Go's core. |
| C# | Not built. | **Yes, on .NET 10+.** `dotnet add package HyperTabular` into a Blazor WebAssembly project links the core beside HyperCast's through the package's `.targets`; it needs the `wasm-tools` workload (`wasm-tools-net10` for a `net10.0` app on the .NET 11 SDK), and the build stops with `HYPERTABULAR002` without it. The archive is built for plain wasm, with no exception-handling instructions and no feature but the two .NET 10's toolchain accepts throughout, so both frameworks link it as it is, Release or Debug. `csharp/HyperTabular.WasmSmokeTest` proves it in headless Chromium on both; see [`csharp/README.md`](../csharp/README.md#webassembly-blazor). |
| Java | Not built. The jar reaches the core through JNI on 21–24 and FFM downcalls on 25+, into the native libraries it bundles for the platform. | Blocked. No Java-to-wasm compiler has FFM or loads a JNI library, the two mechanisms this binding is built on. |
| Ruby | Not built. | **Yes, through `rbwasm build`.** List `hypertabular-wasm` instead of `hypertabular` in the Gemfile handed to ruby.wasm 2.10+ (Ruby 3.4 or 4.0): it carries the Magnus extension prebuilt for `wasm32-wasip1` and shares the interpreter with `hypercast-wasm`. CI links it into both minors' interpreters and runs a smoke test under Node and in headless Chrome, in the weekly and release builds; see [`ruby/README.md`](../ruby/README.md#ruby-in-the-browser). |
| Python | Not built. Platform wheels and the sdist leave no install a wasm backend could fill in for. | **Yes, on [Pyodide](https://pyodide.org/) 314.x.** `await micropip.install("hypertabular")` installs the seventeenth wheel (`pyemscripten_2026_0_wasm32`) and `hypercast`'s; CI runs the whole pytest suite in Pyodide under Node and in headless Chrome. See [`python/README.md`](../python/README.md#in-the-browser-pyodide). |
| Go | The other way round: the binding *is* the core's `wasm32-wasip1` module, translated to Go source by wasm2go and compiled by the Go compiler, so there is no engine and nothing to load. | **Yes, under stock Go and [TinyGo](https://tinygo.org) alike.** `GOOS=js` and `GOOS=wasip1` build under stock Go and `tinygo build -target=wasm` (or `wasip1`) under TinyGo, with no archive and no cgo. CI runs the whole suite under TinyGo on wasip1 under wasmtime, under stock Go on `js/wasm` under Node and on `wasip1/wasm` under wasmtime, and a TinyGo smoke program in headless Chrome; see [`go/README.md`](../go/README.md#webassembly). |
| Swift | Not built. | **Yes, on Swift 6.2+.** swift.org's WebAssembly SDK links the artifact bundle's `wasm32-unknown-wasip1` archive; the result is a `wasm32-wasip1` command module, run in the browser through a WASI shim. CI runs the suite under WasmKit on Swift 6.4, a smoke executable on 6.2, and the same smoke executable in headless Chrome; see [`swift/README.md`](../swift/README.md#webassembly). |
| PHP | Not built. There is no maintained wasm engine PHP can embed. | Not built. |
| TypeScript | **Yes.** The tenth npm package, `@skunkwerkx/hypertabular-wasm32-wasip1`, is the core built for `wasm32-wasip1` by napi-rs and run in-process through Node's `node:wasi` by `@napi-rs/wasm-runtime`, loaded where no platform package fits. CI runs the whole suite under the forced fallback (`NAPI_RS_FORCE_WASI=error`). Bun 1.4.4 cannot initialise it, and on big-endian hosts its process never exits on its own, a printed loss; see [`typescript/README.md`](../typescript/README.md#when-no-native-addon-fits). | **Yes, in Chromium.** The same package's `browser` entry fetches the module and exports the same classes; `typescript/browser-smoke/` replays the whole corpus through it in headless Chrome. Firefox and Safari are not tested. |
| R | Not built. A source install compiles the core for its own platform. | Not attempted. |
| C, C++ | Not applicable: the archive is the core. | **Yes, for wasi-sdk.** The `wasi-wasm32` archive carries the core's `wasm32-wasip1` static library with both headers, static only, the same library Swift's WebAssembly build links and tests. CI packs it and checks its layout but runs nothing against it. |

Arguments are kept to twelve integers or fewer on every export, because Mono's interpreter —
which runs .NET in the browser — passes no more to a native function; the work buffers travel
in one `HyperTabularBuffers` struct instead.
