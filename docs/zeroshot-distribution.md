# Zeroshot distribution contract

Zeroshot uses one explicit release workflow: `.github/workflows/release.yml`. An operator supplies an
`X.Y.Z` version and an exact commit on `main`; the workflow accepts only major version 8 or newer.

## Canonical identity

- Git tag and GitHub Release: `vX.Y.Z`
- executable: `zeroshot` (`zeroshot.exe` on Windows)
- archives: `zeroshot-vX.Y.Z-<target>.tar.gz`
- target manifest: `distribution/zeroshot-targets.json`
- npm package: `@the-open-engine-company/zeroshot`
- target image: `ghcr.io/the-open-engine/zeroshot-target`

No alternate tag prefix, package, image, executable alias, source-build fallback, or compatibility
artifact is published.

## Targets

The authoritative target manifest declares Linux x64/arm64 musl, macOS x64/arm64, and Windows x64.
The npm target table must match it exactly. Unsupported hosts fail closed.

Every archive contains exactly one executable. `SHA256SUMS` contains exactly one entry for every
declared archive. Linux release binaries are checked for a missing dynamic interpreter.

## Version staging

Checked-in manifests use non-authoritative development versions. `scripts/distribution.js
stage-version` updates the release workspace's `zeroshot/Cargo.toml` and `Cargo.lock`; `check-version`
verifies the exact coupling before compilation. Release automation never commits staged versions.

## Publication order

1. Build and smoke all native archives.
2. Generate and verify `SHA256SUMS`.
3. Build and smoke `docker/zeroshot-target/Dockerfile` from the same commit.
4. Create or verify the canonical GitHub Release and exact assets.
5. Publish the image version, source-commit, and eligible `latest` tags.
6. Pack, install, smoke, and publish `@the-open-engine-company/zeroshot`.
7. Publish Python SDK revision `1` from the same canonical release.

See `PUBLISHING.md` for operator setup and recovery rules.
