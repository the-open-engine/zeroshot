# Contributing

Zeroshot v8 is a native Rust product with a Python SDK and a small Node.js repository-tooling layer.
There is no legacy Node runtime in this repository.

## Prerequisites

- Rust 1.97.0 with `rustfmt` and `clippy`
- Node.js 24 and npm for repository tooling and release scripts
- Python 3.12 for SDK work
- Docker for target-image changes

## Setup

```bash
npm ci
cargo test --workspace
python -m venv sdks/python/.venv
sdks/python/.venv/bin/python -m pip install -e 'sdks/python[dev]'
```

## Repository map

| Area                              | Path                                         |
| --------------------------------- | -------------------------------------------- |
| Canonical executable and engine   | `zeroshot/`                                  |
| Shared cluster protocol           | `crates/openengine-cluster-protocol/`        |
| Cluster server/client             | `crates/openengine-cluster-{server,client}/` |
| Protocol fixtures and conformance | `crates/openengine-cluster-testkit/`         |
| npm installer/launcher            | `npm/zeroshot/`                              |
| Target image                      | `docker/zeroshot-target/`                    |
| Python SDK                        | `sdks/python/`                               |
| Release tooling                   | `scripts/distribution.js`                    |
| Repository tooling tests          | `tests/tooling/`                             |

## Validation

Start with the narrowest relevant command, then run broader checks before handoff.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps

npm run lint
npm test
npm run distribution:check
npm run protocol:check

cd sdks/python
python -m ruff check src tests examples
python -m ruff format --check src tests examples
python -m mypy src examples
python -m pytest
```

Use `npm run check` for the complete repository-tooling gate. Docker changes should also build and
smoke `docker/zeroshot-target/Dockerfile`.

## Generated files

Rust protocol types and fixtures are authoritative. Do not hand-edit generated artifacts under
`protocol/openengine-cluster/v1/`. Regenerate/check them through the testkit generator.

CLI Markdown and HTML are generated from the Clap model:

```bash
cargo run -p zeroshot --example generate_cli_docs -- --write
cargo run -p zeroshot --example generate_cli_docs -- --check
```

## Pull requests

- Target `main`.
- Use a Conventional Commit header as the PR title; squash merge makes it the released commit.
- Keep changes scoped and include focused tests.
- Do not commit release versions or create version tags manually.
- Do not reintroduce legacy Node product paths, compatibility aliases, state migration, or dual
  publication identities.

The stable required check is `CI / required`.
