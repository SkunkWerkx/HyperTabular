# npm consumer tests

What a consumer gets from the ten npm tarballs, proven by installing them. `run.sh` serves the
packed tarballs from a local [Verdaccio](https://verdaccio.org) registry on port 4873, then
runs each scenario in a fresh directory under `work/<scenario>/` with its own `package.json`,
`.npmrc` and npm cache, so `npm install` resolves, selects platforms and writes lockfiles
exactly as it would against npmjs. Nothing here reaches npmjs for the packages under test:
`@skunkwerkx/hypertabular*` has no uplink in `verdaccio.yaml`. The peer,
`@skunkwerkx/hypercast*`, and everything else (the WASI package's runtime, the bundlers'
dependencies) proxy through; until the HyperCast version the package names is on npmjs,
`HYPERCAST_TARBALLS=<dir>` publishes HyperCast's own packed tarballs into the local registry
first. `smoke.cjs` is the consumer: it requires `@skunkwerkx/hypertabular`, prints
`available=<bool> target=<native|wasm32-wasip1|none>`, reads the `u64` maximum and a cell that
fails its door (its span in the raw bytes, and the peer's own door agreeing), a contract
violation, one structural failure from `delimited.json` and one sheet from `workbook.json`
(both from `HYPERTABULAR_CORPUS`). When nothing loaded it prints the
`ERR_HYPERTABULAR_UNAVAILABLE` error's cause chain, one line per cause. `bundled.cjs` is its
public-API half, for the bundler scenario.

The header of `run.sh` lists the scenarios with what each asserts. The container scenarios
(`floor`, `s390x`) need Docker; every other scenario, `mac-lockfile` included, runs on any
host.

## Running locally

Pack tarballs from this machine's one native and the WASI build (from the repository root;
`npm run build` then `npm run build:wasi` in `typescript/` first):

```sh
mkdir -p /tmp/node-artifacts/node-osx-arm64 /tmp/node-artifacts/node-wasm32-wasip1
cp typescript/hypertabular.darwin-arm64.node /tmp/node-artifacts/node-osx-arm64/
cp typescript/hypertabular.wasm32-wasip1.wasm typescript/hypertabular.wasip1* /tmp/node-artifacts/node-wasm32-wasip1/
python3 ../HyperForge/.github/scripts/npm_package.py pack --package typescript \
  --artifacts /tmp/node-artifacts --out /tmp/npm-local --allow-missing-natives
```

Then the host scenarios:

```sh
typescript/consumer-tests/run.sh /tmp/npm-local negative-unpublished plain omit-optional \
  absent-native forced-wasi negative-deleted-package negative-deleted-wasm mac-lockfile bundlers
```

And the container scenarios, which need Docker (on Docker Desktop the containers reach the
registry through `host.docker.internal`; on a Linux host leave `REGISTRY_HOST` unset and they
share the host network):

```sh
REGISTRY_HOST=host.docker.internal typescript/consumer-tests/run.sh /tmp/npm-local floor
REGISTRY_HOST=host.docker.internal typescript/consumer-tests/run.sh /tmp/npm-local s390x
```

Negative controls: `CORPUS_DIR=$(mktemp -d) run.sh <dir> plain` fails on the corpus read;
`FLOOR_IMAGE=node:22.12.0-alpine run.sh <dir> floor` fails the install with `EBADENGINE`.

## What the runs established (2026-11-01, Apple M6, Node 26.13.0, npm 11.20.0, Docker 29.9.2)

Every scenario passed from a local pack of the darwin-arm64 addon and the WASI
module, with HyperCast's own darwin-arm64 and WASI packages published beside them through
`HYPERCAST_TARBALLS`, and both negative controls failed as stated (an empty `CORPUS_DIR` on the
corpus read, the 22.12.0 image with `EBADENGINE`). The floor scenario loaded `wasm32-wasip1` on
`node:22.13.0-alpine` with engine-strict, `node:22-alpine` and `node:24-alpine`, since a local
pack carries no musl addon.

- **The peer.** Every install brought `@skunkwerkx/hypercast` in beside the package as npm 7
  and later do for a peer, and on every backend the peer's door agreed with the cell's
  verdict. Where the package fell back to WebAssembly, so did the peer.
- **Bundlers.** Neither esbuild (`--bundle --platform=node`) nor webpack produces a working
  self-contained bundle: webpack fails the build on the loader's platform `require`s, and
  esbuild bundles but the bundle cannot load the module from a directory with no
  `node_modules`. The configuration that passes, for both, is `@skunkwerkx/hypertabular`
  **external**, installed beside the bundle. That is the configuration the package README
  documents.
- **npm bug 4828.** `mac-lockfile` reproduces npm/cli#4828 with npm 10.9.2, the npm Node
  22.13.0 bundles: a lockfile regenerated beside a darwin-arm64 `node_modules` names only that
  platform, and `npm ci --os=linux --cpu=x64` from it installs no native and loads the
  fallback. Node 22 users with a lockfile a Mac wrote are who the fallback is for.
- **Bun.** Under `NAPI_RS_FORCE_WASI=error`, Bun 1.4.4 cannot initialise the WASI module (its
  `node:wasi` has no reactor `initialize`), so `require('@skunkwerkx/hypertabular')` succeeds
  with `isAvailable() === false`, printed as `bun: ...` and never failing the run. The native
  addon on Bun passes the whole suite.
- **Big-endian hosts.** In `node:24-alpine` for `linux/s390x` under QEMU the install resolves
  no native package, the fallback loads (the peer's too) and every check of `smoke.cjs`
  passes, and the process then never exits (`smoke exit=143`, killed by the bound): the
  runtime writes the module's threadsafe-function reference count little-endian and reads it
  back host-endian, so the count never reads as zero and the `MessagePort` holding the event
  loop open is never released. `container/consumer.sh` bounds the smoke with `timeout 300`
  and prints `smoke exit=<status>`, and `scenario_s390x` asserts, after the checks, that the
  bound killed it: an exit of 0 means upstream fixed it and the docs are stale, so it fails
  too. Nothing beyond the smoke's checks is claimed for big-endian hosts.
- **Publishing to Verdaccio.** npm refuses to publish with no credentials at all, whatever the
  registry's rule, so `run.sh` creates one user through the registry's own user endpoint
  (`max_users: 1` in `verdaccio.yaml`) and puts its token in a user config of its own
  (`NPM_CONFIG_USERCONFIG=work/npmrc`), so the developer's `~/.npmrc` never reaches a scenario.
- **Project config.** npm reads the project `.npmrc` from the nearest directory holding a
  `package.json` or `node_modules`, so every consumer directory gets a minimal `package.json`
  first; without it the harness's own manifest would be the project.
