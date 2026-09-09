# Zeroshot distribution contract

`.github/workflows/release.yml` is the only Zeroshot release workflow. An operator supplies an
`X.Y.Z` version with an exact commit on `main`; the workflow rejects versions before major 8.

## Canonical identity

| Release item         | Identity                                                                 |
| -------------------- | ------------------------------------------------------------------------ |
| Git tag and release  | `vX.Y.Z`                                                                 |
| Executable           | `zeroshot` (`zeroshot.exe` on Windows)                                   |
| Native archives      | `zeroshot-vX.Y.Z-<target>.tar.gz`                                        |
| Target manifest      | `distribution/zeroshot-targets.json`                                     |
| npm package          | `@the-open-engine-company/zeroshot`                                      |
| Target image         | `ghcr.io/the-open-engine/zeroshot-target`                                |
| Python wheel release | `zeroshot-python-vX.Y.Z_1`                                               |
| Python package       | `the-open-engine-zeroshot==X.Y.Z.post1` when PyPI publication is enabled |
| Docs snapshot        | `vX.Y.Z/` relative to the docs base, including Python SDK revision `1`   |

The workflow publishes no alternate tag prefix, package, image, executable alias, source-build
fallback, or compatibility artifact.

## Targets

The target manifest declares Linux x64/arm64 musl, macOS x64/arm64, and Windows x64, and the npm
target table must match it exactly. The installer stops on any other host.

Every archive contains one executable, with one corresponding entry in `SHA256SUMS`. The workflow
also rejects a Linux release binary that has a dynamic interpreter.

## Version staging

Checked-in manifests carry development versions that are not release identifiers. In the release
workspace, `scripts/distribution.js stage-version` updates `zeroshot/Cargo.toml` and `Cargo.lock`,
then `check-version` verifies their coupling before compilation. Release automation does not commit
those staged versions.

## Publication order

| Step | Release action                                                                                         |
| ---- | ------------------------------------------------------------------------------------------------------ |
| 1    | Build and smoke all native archives.                                                                   |
| 2    | Generate and verify `SHA256SUMS`.                                                                      |
| 3    | Build and smoke `docker/zeroshot-target/Dockerfile` from the same commit.                              |
| 4    | Create or verify the canonical GitHub Release and its exact assets.                                    |
| 5    | Publish the image version, source-commit, and eligible `latest` tags.                                  |
| 6    | Pack, install, smoke, and publish `@the-open-engine-company/zeroshot`.                                 |
| 7    | Build and publish Python SDK revision `1` as an immutable GitHub wheel release.                        |
| 8    | Publish that wheel set to PyPI; use `publish_pypi: false` only when trusted publishing is unavailable. |
| 9    | Publish the immutable docs snapshot and advance `stable` for the newest release.                       |

`PUBLISHING.md` contains the operator setup and recovery rules.
