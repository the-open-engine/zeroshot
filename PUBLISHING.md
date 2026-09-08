# Zeroshot publishing

Zeroshot has one canonical release train. The explicit `.github/workflows/release.yml` workflow
publishes an exact commit from `main` as `vX.Y.Z`; v8 is the first release on this contract.

## Published outputs

One successful release produces the same version across:

- GitHub tag and Release: `vX.Y.Z`
- native archives: `zeroshot-vX.Y.Z-<target>.tar.gz`
- checksum manifest: `SHA256SUMS`
- npm package: `@the-open-engine-company/zeroshot@X.Y.Z`
- target image: `ghcr.io/the-open-engine/zeroshot-target:X.Y.Z`
- target image source tag: `sha-<full-commit>`
- Python revision 1 GitHub wheel release: `zeroshot-python-vX.Y.Z_1`
- Python package when PyPI publication is enabled: `the-open-engine-zeroshot==X.Y.Z.post1`

The checked-in Cargo and npm versions are development placeholders. The release workspace stages the
explicit version and never commits it back to `main`.

## Release prerequisites

1. The exact commit is an ancestor of `origin/main`.
2. The commit has a successful `CI / required` check.
3. The requested version is `8.0.0` or newer and is greater than the highest canonical `vX.Y.Z` tag.
4. The `release` GitHub environment permits the publishing jobs.
5. npm trusted publishing is configured for `@the-open-engine-company/zeroshot` and
   `.github/workflows/release.yml`.
6. When `publish_pypi` is enabled, PyPI trusted publishing is configured for
   `the-open-engine-zeroshot` and `.github/workflows/release-python.yml`.
7. GitHub Packages permits publishing `ghcr.io/the-open-engine/zeroshot-target`; make the package
   public after its first publication when anonymous pulls are required.

Do not add long-lived npm or PyPI tokens. Publishing uses GitHub OIDC trusted publishing.

## Run a dry run

Dispatch `Release Zeroshot` with:

- `action`: `dry-run`
- `version`: the intended `X.Y.Z`
- `release_commit`: the exact 40-character commit SHA

The workflow builds every native archive, verifies checksums and static Linux binaries, builds and
smokes the target image, packs the npm package, and performs `npm publish --dry-run`. It does not
create tags or publish registries.

## Publish

Dispatch the same workflow with `action: release`. `publish_pypi` defaults to `true`. Set it to
`false` only when PyPI trusted publishing is known to be unavailable; this is an explicit deferral,
not a blanket ignored failure. The workflow:

1. verifies the exact source and canonical version ordering;
2. builds the five declared native targets;
3. creates and verifies `SHA256SUMS`;
4. builds the canonical target image from the same source;
5. creates or verifies the GitHub Release and uploads exact artifacts;
6. publishes immutable image tags and `latest` when this is the newest release;
7. packs, installs, smokes, and publishes `@the-open-engine-company/zeroshot`;
8. invokes the Python SDK workflow for revision `1`, always creating or verifying its immutable
   GitHub wheel release and publishing the same wheels to PyPI when `publish_pypi` is enabled.

Later Python-only revisions may dispatch `Release Python SDK` with the same Zeroshot version, a
higher positive SDK revision, and an exact `main` commit containing the SDK changes.

## Deferred PyPI publication

When the PyPI organization or trusted publisher is not yet available, dispatch either release
workflow with `publish_pypi: false`. The Python wheels are still built, verified, and attached to the
revision's GitHub Release, and the workflow records a warning and successful intentional deferral.

After PyPI becomes available, dispatch `Release Python SDK` with `action: release`,
`publish_pypi: true`, and the same Zeroshot version, SDK revision, and release commit. Recovery
verifies every existing immutable wheel before publishing the exact set to PyPI.

## One-time npm bootstrap

If the scoped npm package does not yet exist, an npm organization owner must create
`@the-open-engine-company/zeroshot` and configure the trusted publisher before the automated release
can publish. Use the exact packed artifact from a reviewed workflow if npm requires an initial
interactive publish; do not create a second package name or temporary compatibility package.

## Recovery

Release jobs are designed to verify already-published immutable artifacts before completing missing
steps. Recovery must use the same version, tag, and source commit. Never overwrite a different npm
tarball, GitHub asset, Python wheel, image source label, or tag target.
