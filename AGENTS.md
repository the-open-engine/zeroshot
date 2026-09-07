UPDATE THIS FILE when making architectural changes, adding patterns, or changing conventions.

# Zeroshot v8

Operational guidance for automated agents working on this repository. Install the canonical command
with `npm i -g @the-open-engine-company/zeroshot` or build `zeroshot` with Cargo.

## Critical rules

- Never run `zeroshot run` unless the user explicitly asks to start a run.
- Never use git commands inside validator prompts; validators inspect files and observable outputs.
- Agents are non-interactive. Make autonomous, scoped decisions rather than asking runtime questions.
- Never edit `CLAUDE.md` unless the user explicitly requests it.
- `main` is the only development and release trunk. Normal PRs target `main`.
- Pull request titles are Conventional Commit headers because squash merge makes the title the
  released commit.
- Worker git operations are allowed only inside an isolated worktree/container or explicit PR/ship
  delivery flow.
- Do not recreate the retired Node.js product, its commands, configuration, state, release workflow,
  package exports, compatibility aliases, migration, or dual publication identities.

## Product and release identity

- The Rust crate in `zeroshot/` is the canonical product and owns the `zeroshot` CLI.
- Node.js exists only for repository tooling and the npm binary delivery package.
- Canonical releases are explicit `vX.Y.Z` tags with major version 8 or newer.
- The npm package is `@the-open-engine-company/zeroshot`.
- The target image is `ghcr.io/the-open-engine/zeroshot-target`.
- The Python distribution is `the-open-engine-zeroshot`; its import package remains `zeroshot`.
- Python SDK tags are `zeroshot-python-vZEROSHOT_SDK` and package versions are
  `ZEROSHOT.postSDK`. Canonical releases publish SDK revision `1`; later SDK revisions may release
  independently from an exact `main` commit descended from the canonical tag.
- Checked-in Cargo/npm versions are development placeholders. Tags, registry metadata, and GitHub
  Releases are authoritative. Never commit a staged release version to `main`.
- Release recovery may complete missing outputs only when existing immutable artifacts match the
  exact version and source commit.

## Runtime invariants

- Protocol Rust types are the source of truth. Generated files under
  `protocol/openengine-cluster/v1/` must be regenerated through the Rust testkit, not hand-edited.
- Model identifiers are opaque provider-owned strings. Do not infer a harness from a provider/model,
  maintain model catalogs, or validate provider availability. Admission may reject only known
  incompatible harness/provider pairs.
- Runtime selection requires caller-authored `harness`, `provider`, and `model` values.
- Structured-output recovery is provider-owned and fail-closed. A recovery turn must disable reused
  provider sessions, MCP, approval bypass, write/network tools, and user-defined agents/config.
- Provider continuation is bounded: Claude continues once after `system/api_retry`; Codex continues
  once after a terminal execution error. Both send literal `Continue` in the same session when one
  exists. Structured output receives at most two correction turns before `malformed`.
- Provider JSONL readers do not cap cumulative output. They share only the 64 MiB unfinished-record
  guard, accept a complete final record without a newline, ignore unknown future event types before
  validating provider-owned fields, and continue draining after the first valid terminal event.
- Provider stdin and stdout are concurrent and bounded so large prompts and early output cannot
  deadlock. Incomplete stdin is fatal, while parsed identity, usage, retry, and diagnostics survive
  either I/O completion order.
- Durable provider events cross bounded async queues with backpressure. Cancellation preserves token
  usage and event order; overflow is explicit and produces an incomplete marker rather than silent
  loss.
- Safe-log timestamps are captured at the producer boundary as positive JavaScript-safe Unix epoch
  milliseconds and remain unchanged across durable replay.
- Run close reserves and tombstones execution activity atomically. No late start, handle, or stream
  may surface after close returns.
- Contained provider sessions bound post-exit I/O draining by the command deadline and a ten-minute
  ceiling while still observing cancellation and cleanup.
- GitHub delivery treats aggregate merge policy and required contexts as authority, waits through
  merge queues/deferrals, and succeeds only after observing the exact merged result. Outside merge
  queues, branch freshness advances only through an authorized compare-and-swap response.

## CLI and target contracts

- CLI grammar/help comes from the derived Clap `Cli` tree and Rust doc comments.
- Do not hand-edit `docs/zeroshot-cli.md` or `docs/zeroshot-cli.html`; regenerate with
  `cargo run -p zeroshot --example generate_cli_docs -- --write` and verify with `--check`.
- The direct target's discovery, sourceful run request, and run-scoped OECP session are versioned
  native-v2 protocol contracts. Do not add alternate endpoints as aliases.
- Target bootstrap keys are one-time file inputs. Secret values never enter run ledgers, target
  configuration, or observation records.
- Read-only safe commands include `zeroshot list`, `zeroshot status`, and `zeroshot logs`.
- Destructive commands such as `zeroshot force-stop` require explicit user intent.

## Where to look

| Concept                       | Path                                                                                           |
| ----------------------------- | ---------------------------------------------------------------------------------------------- |
| Canonical crate and CLI       | `zeroshot/`                                                                                    |
| CLI grammar/help              | `zeroshot/src/native_v2_cli/parser.rs`                                                         |
| CLI composition               | `zeroshot/src/native_v2_cli.rs`, `zeroshot/src/main.rs`                                        |
| Built-in templates            | `zeroshot/src/native_v2_templates.rs`, `zeroshot/src/native_v2_templates/`                     |
| Local run composition         | `zeroshot/src/native_v2_local.rs`                                                              |
| Hosted/cloud composition      | `zeroshot/src/native_v2_cloud.rs`, `zeroshot/src/native_v2_hosting.rs`                         |
| Portable controller           | `zeroshot/src/native_v2_portable_controller.rs`, `zeroshot/src/native_v2_portable_controller/` |
| Provider/delivery composition | `zeroshot/src/native_v2_candidate.rs`, `zeroshot/src/native_v2_candidate/`                     |
| Target server                 | `zeroshot/src/native_v2_target.rs`, `zeroshot/src/native_v2_target/`                           |
| Target authority/auth         | `zeroshot/src/native_v2_target_authority.rs`, `zeroshot/src/native_v2_target_authority/`       |
| Contained execution           | `zeroshot/src/execution.rs`, `zeroshot/src/execution/`                                         |
| Faults and redaction          | `zeroshot/src/fault.rs`, `zeroshot/src/fault/`                                                 |
| Run ledger                    | `zeroshot/src/v2_run_ledger.rs`, `zeroshot/src/v2_run_ledger/`                                 |
| Cluster protocol types        | `crates/openengine-cluster-protocol/`                                                          |
| Cluster server                | `crates/openengine-cluster-server/`                                                            |
| Cluster client                | `crates/openengine-cluster-client/`                                                            |
| Conformance fixtures          | `crates/openengine-cluster-testkit/`                                                           |
| Generated protocol artifacts  | `protocol/openengine-cluster/v1/`                                                              |
| npm package                   | `npm/zeroshot/`                                                                                |
| Target image                  | `docker/zeroshot-target/`                                                                      |
| Target declarations           | `distribution/zeroshot-targets.json`                                                           |
| Distribution tooling          | `scripts/distribution.js`                                                                      |
| Python SDK                    | `sdks/python/`                                                                                 |
| Release workflow              | `.github/workflows/release.yml`                                                                |
| Python release workflow       | `.github/workflows/release-python.yml`                                                         |
| CI classifier                 | `.github/ci-path-classifier.js`                                                                |
| Repository tooling tests      | `tests/tooling/`                                                                               |

## Development conventions

- Fix root causes and keep changes scoped.
- Use existing patterns; do not add parallel registries, provider lists, model catalogs, or release
  authorities.
- New Rust APIs must respect the four-parameter Clippy ceiling; use request structs rather than
  raising or bypassing the limit.
- Preserve bounded values, explicit overflow, cancellation safety, and exact source provenance at
  every public boundary.
- Add focused tests beside the owning crate/module. Do not delete Rust-owned legacy protocol
  fixtures merely because their names describe older wire participants.
- Update this file whenever architecture, ownership, release identity, or conventions change.

## Validation

Run the narrowest relevant checks first, then the complete affected lane.

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

For graph-sensitive or introduced-change work, run:

```bash
npm run opcore:status
opcore-zero check --repo . --changed --json
opcore-zero sense --repo . --json
npm run opcore:graph:update
```

## Release convention

- CI has native, Python, and repository-tooling lanes plus stable aggregate `required`.
- `.github/workflows/release.yml` is the only canonical product release workflow.
- It publishes native archives/checksums, `ghcr.io/the-open-engine/zeroshot-target`, and
  `@the-open-engine-company/zeroshot`, then invokes Python revision `1`.
- `.github/workflows/release-python.yml` may publish later SDK-only revisions.
- There is no automatic semantic release, release-promotion branch, `dev -> main` flow, or second
  runtime release train.
