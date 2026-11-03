# HyperTabular release flow

Follow the [shared runbook](https://github.com/SkunkWerkx/.github/blob/master/docs/release-runbook.md),
its "The release train" section. The workflows call the Forge at master; the preparation run
resolves that to one commit and records it in the package inventory, and the tag run requires
the Forge it uses to descend from it. Hold Forge merges while a preparation is in flight.

HyperCast releases first: HyperTabular's release takes HyperCast from the registries, at the
version its manifests require, never from the pin (the runbook's "Release order").

0. Release HyperCast, with the Forge's `finalize-release` attaching its CRAN tarball to its
   GitHub Release. Then raise every manifest's HyperCast requirement to that version (the list
   is CLAUDE.md's "Upstream: HyperCast") and check them from this checkout:

   ```sh
   python3 ../HyperForge/.github/scripts/companion_gate.py --root .
   ```

   It fails when the manifests disagree, or when the version is not yet on crates.io, NuGet,
   Maven Central, PyPI, RubyGems, npm, the Go proxy, Packagist and HyperCast's GitHub Release
   (the PIE zips, the Swift zips, the C archives and the CRAN tarball), naming every missing
   item. CRAN is not asked.
1. Write the bullets under `## [Unreleased]` in `CHANGELOG.md` (or the version's heading) for
   every package the diff touched, and fill every `⟨…⟩` placeholder. Nothing else about the
   release is prose: `prepare-release.yml` dates the heading and sets `r/NEWS.md`'s, and
   refuses a section that is missing or still holds a placeholder, before it pushes anything.
2. Dispatch `prepare-release.yml` with the bare version. It bumps every manifest, regenerates
   the Go core at the version (`go/internal/core/core.go`, whose version is a literal), pushes
   A and runs `ci.yml` on it in release mode. Its `companions` job takes no pin there: it
   refuses any companion override or `.companions/` in the tree and runs the same gate, and
   every board job waits for it, so a HyperCast not yet live everywhere but CRAN holds the
   whole run, listing what is missing; re-run the failed jobs once it is. After the board, that
   run's tail packs every registry input from its own artifacts, verifies each file's build
   provenance, builds and attests the inventory, preserves `release-packages` for 90 days,
   dispatches `codeql.yml` on A and requires a clean code scanning dashboard, and commits B on
   master: the Swift zips' checksums and the version in `Package.swift`, with the
   `Release-Source-SHA`, `Release-CI-Run-ID` and `Release-Forge-SHA` trailers. No binary is
   committed.
3. Read the run's summary: B's trailers, the inventory, the digest listing, CodeQL clean.
   Nothing may be pushed to master between B and the tag; if anything was, prepare again.
4. Push the root tag `vX.Y.Z` at B, annotated and signed (`git tag -m "HyperTabular vX.Y.Z"
   vX.Y.Z <B>`, with `tag.gpgsign` on). The tag run holds the tag to B and
   B to its run, downloads the preserved packages, re-verifies every file and the inventory at
   A, holds B's `Package.swift` to the preserved zips, signs the release attestation from the
   tag, and publishes to the six registries, each publisher verifying its files just before
   uploading. It then creates `go/vX.Y.Z` and the GitHub Release, with the 18 PHP extension
   zips attached (PIE installs from there; Packagist sees the tag at once, so `pie install`
   fails until the Release exists), and its last job, `pie-acceptance`, has PIE install
   `skunkwerkx/hypertabular:vX.Y.Z` on every platform and PHP minor and checks it from a fresh
   `php`.
5. Read the tag run's summary: six registries confirmed, the Release written, PIE green. Then
   dispatch `release.yml` at the tag once with that run's id:

   ```sh
   gh workflow run release.yml --repo SkunkWerkx/HyperTabular --ref vX.Y.Z -f publication_run_id=<tag run id>
   ```

   Every publisher must report nothing to do. That is the replay proof, so every release
   still exercises recovery, as a no-op instead of as the normal path.
6. Submit the R package to CRAN by hand, after HyperCast's own submission at the version it
   requires has been accepted: download `release-packages` from the tag run and upload its
   `cran/hypertabular_X.Y.Z.tar.gz`, the attested bytes. No job uploads it. Both preparations'
   `release-packages` last 90 days, which covers both reviews; until then R users install
   either package from its GitHub Release.

Every pull request, the weekly schedule and a `purpose=board` dispatch test against the HyperCast
commit and green run `.github/hyper.json`'s `companions` block pins, never against HyperCast's
registries. To move the pin (the runbook's "Moving a companion pin"):

1. Get a pinnable HyperCast run: a `board` dispatch at master,
   `gh workflow run ci.yml --repo SkunkWerkx/HyperCast --ref master -f purpose=board`, a green
   scheduled run, or the green `purpose=release` run that prepared one of its releases.
2. On a branch here: `python3 ../HyperForge/.github/scripts/companion_pin.py --root .
   --companion hypercast --run <run id>`. It refuses a run that is not a green full board at a
   commit on HyperCast's master, naming why, rewrites the pin's `commit` and `run`, and prints
   the commits between the old pin and the new one for the pull request's description.
3. Open the pull request; its board takes the new pin.

A run's artifacts expire after 90 days, and the `companions` job then fails with "re-run
`SkunkWerkx/HyperCast` CI at `<sha>` with `purpose=board` and move the pin": dispatch a new
`board` run and move the pin to it. A fork's pull request gets no companion secret and fails
the same job at once; a maintainer re-runs it from a branch here.

Verify one file of each kind from the tag alone, exactly as a consumer would:

```sh
gh attestation verify <file> --repo SkunkWerkx/HyperTabular --source-ref refs/tags/vX.Y.Z \
  --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Before 1.1.0 only: the ten npm placeholders and their trusted publishers. An npm package
that does not exist yet (the first release, for all ten names, or a new platform) has no
trusted publisher to accept the upload, so the owner bootstraps it as the shared runbook says:
a placeholder `0.0.1` under each new name, published by hand with a granular token, then
`release.yml` registered as its trusted publisher and the token revoked. The release version
itself is never published by hand, so its first real version is the tag run's Trusted
Publishing upload and carries npm provenance. Also a glance at the Central Portal's usage page.

The retry ladder: rerun the failed jobs on the same run; then dispatch at the tag with the
receipts (`publication_run_id`, and `maven_deployment_id` after an ambiguous Central upload),
which verifies the tag run's release attestation instead of making one and pushes nothing
twice; then, for a wrong artifact, a patch release. Never a retag, a repack or a re-attest:
registry versions can never be replaced, and the release attestation names every digest. A
preparation that failed, or whose `release-packages` expired (90 days), is prepared again from
a new `prepare-release.yml` dispatch; the tag guard refuses a B whose run has nothing
preserved. NuGet symbols and ambiguous Central uploads require the receipt/deployment
reconciliation described in the shared runbook.

A red `pie-acceptance` after publication is fixed with a patch release, never in place: no
renamed asset (the inventory records its path), no repacked zip (the release attestation
names its digest) and no re-attest. Re-run the failed leg once if the failure was transient;
otherwise fix the cause and release X.Y.Z+1, noting in the CHANGELOG that X.Y.Z's PHP zips
don't install.
