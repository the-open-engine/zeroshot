# Zeroshot v8 — Claude Guidance

`AGENTS.md` is authoritative. Follow it before this file.

## Product

- Zeroshot v8 is the Rust product in `zeroshot/` and owns the `zeroshot` CLI.
- Install with `npm i -g @the-open-engine-company/zeroshot`.
- Node.js is repository tooling and npm binary delivery only.
- Python distribution: `the-open-engine-zeroshot`; import: `zeroshot`.
- Target image: `ghcr.io/the-open-engine/zeroshot-target`.
- Releases use explicit `vX.Y.Z` tags with major version 8 or newer.

Do not restore the retired Node product, compatibility aliases, migration, legacy commands, dual
release trains, semantic-release, or a `dev` branch workflow.

## Non-Negotiable Rules

- Never run `zeroshot run` without explicit user permission.
- Agents are non-interactive; make scoped autonomous decisions.
- Normal development and release work targets `main`.
- Use git only inside the current isolated worktree. Never use git inside validator prompts.
- Preserve exact source provenance, bounded values, cancellation safety, and explicit overflow.
- Treat provider/model identifiers as opaque. Do not maintain model catalogs or infer harnesses.
- Protocol Rust types are authoritative; regenerate protocol artifacts through the Rust testkit.
- Regenerate CLI docs with the Rust example; never hand-edit generated CLI Markdown or HTML.
- Keep release recovery fail-closed: existing immutable outputs must match exact source bytes.

## Repository Map

| Area                 | Path                                   |
| -------------------- | -------------------------------------- |
| Product and CLI      | `zeroshot/`                            |
| Protocol crates      | `crates/`                              |
| Protocol fixtures    | `protocol/openengine-cluster/v1/`      |
| npm package          | `npm/zeroshot/`                        |
| Python SDK           | `sdks/python/`                         |
| Target image         | `docker/zeroshot-target/`              |
| Target manifest      | `distribution/zeroshot-targets.json`   |
| Distribution tooling | `scripts/distribution.js`              |
| CI                   | `.github/workflows/ci.yml`             |
| Product release      | `.github/workflows/release.yml`        |
| Python release       | `.github/workflows/release-python.yml` |
| Tooling tests        | `tests/tooling/`                       |

## Validation

Run the narrowest relevant checks, then the complete affected lanes:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps

npm run check
npm run format:check
actionlint .github/workflows/*.yml

cd sdks/python
python -m ruff check src tests examples
python -m ruff format --check src tests examples
python -m mypy src examples
python -m pytest
python -m mkdocs build --strict
```
