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
- Canonical releases always publish revision `1` wheels to an immutable GitHub Release.
  `publish_pypi` defaults to `true`; operators may set it to `false` only when PyPI trusted
  publishing is known to be unavailable, then recover the same revision from the same source later.
- Checked-in Cargo/npm versions are development placeholders. Tags, registry metadata, and GitHub
  Releases are authoritative. Never commit a staged release version to `main`.
- Release recovery may complete missing outputs only when existing immutable artifacts match the
  exact version and source commit.

## Runtime invariants

- Protocol Rust types are the source of truth. Generated files under
  `protocol/openengine-cluster/v1/` must be regenerated through the Rust testkit, not hand-edited.
- Initial input remains caller-owned and is validated unchanged. A root group may add required
  state fields only when their payload types have deterministic implicit empty values; verification
  proves this and reduction materializes missing null, string, and recursively empty record values.
- Model identifiers are opaque provider-owned strings. Do not infer a harness from a provider/model,
  maintain model catalogs, or validate provider availability. Admission may reject only known
  incompatible harness/provider pairs.
- Runtime selection requires caller-authored `harness`, `provider`, and `model` values.
- Named targets store only endpoint, access mode, and login identity. Named runs resolve repository, branch, exact remote revision, and worktree dirtiness client-side from the invoking Git worktree plus per-run overrides; target records never bind repositories.
- Portable worker bindings resolve through the generic `WorkerRegistry` boundary. External binding
  protocol, version, and profile values are bounded opaque strings; the protocol crate must not
  keep an external binding catalog. `openengine.worker.builtin/v1` is reserved for native
  in-process workers. No portable external binding currently ships; do not reintroduce retired
  worker profiles.
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
  queues, branch freshness advances only through an authorized compare-and-swap response. A
  reported conflict is routable only after the trusted lane fetches the exact current target and
  leaves a verified nonempty Git merge conflict in the workspace; repair agents receive no GitHub
  credential. Merge receipts preserve GitHub's authoritative merged revision.
- Delivery-enabled software-change templates make the acceptance verifier the sole author of the
  current change title and description after every review pass. Git delivery uses that manifest for
  commits and reviews, refreshes only its marker-delimited body section while preserving surrounding
  text. Verifier guidance reserves closing references for delivery, which appends them only from
  caller-owned issue input.
- GitHub review creation and rediscovery are shared by pull-request and merge delivery. They verify
  the exact pushed ref and head, retry bounded transient visibility or API failures, refresh a
  dynamic credential once on HTTP 401/403 within the synchronization deadline and cancellation
  boundary, and fail closed on identity mismatch or static-token rejection. Verifier-authored pull
  request descriptions and source-issue closing references stay inside the generated body markers
  so refreshing metadata cannot retain a stale issue reference. Reviews with an unowned closing
  reference in a legacy Zeroshot layout fail closed instead of rewriting ambiguous human text.

## CLI and target contracts

- CLI grammar/help comes from the derived Clap `Cli` tree and Rust doc comments.
- Do not hand-edit `docs/zeroshot-cli.md` or `docs/zeroshot-cli.html`; regenerate with
  `cargo run -p zeroshot --example generate_cli_docs -- --write` and verify with `--check`.
- The public documentation site is the root `mkdocs.yml`. Python API pages are generated from the
  curated SDK exports and docstrings. Do not restore a second SDK-only MkDocs site.
- Keep `docs/reference/python/*.md` out of Prettier; its Markdown formatter removes the indentation
  required by mkdocstrings directives. Rendered-symbol CI checks are the contract.
- `docs/reference/cluster/api.md` is generated from the final OpenRPC value
  through the Rust testkit. Do not hand-edit it or add a parallel method registry.
- Published documentation uses immutable `vX.Y.Z/` snapshots relative to the docs base, moving
  `stable` and `dev` aliases, and a per-snapshot `manifest.json` with exact source identity and
  logical routes.
- The direct target's discovery, sourceful run request, and run-scoped OECP session are versioned
  native-v2 protocol contracts. Do not add alternate endpoints as aliases.
- Secret-bearing target inputs never enter run ledgers, target configuration, or observation records.
- Target HTTP failures use the shared bounded `{code,message,details?}` protocol problem; message-only
  bodies are invalid, and details contain only user-safe structured metadata.
- Operator diagnostics are private-capability-only, run-scoped, bounded, sanitized, and excluded
  from public run status and logs.
- Hosted merge plans are atomic, immutable, merge-only DAGs over one explicit repository, branch,
  and profile. The target resolves each node's exact revision only after its dependencies succeed;
  plans have static inputs, no cross-node dataflow, and no retry-in-place. Agent runtime bindings
  cannot declare `GH_TOKEN`; the sole merge-delivery binding owns the GitHub write credential.
- Read-only safe commands include `zeroshot list`, `zeroshot status`, and `zeroshot logs`.
- Destructive commands such as `zeroshot force-stop` require explicit user intent.

## Where to look

| Concept                       | Path                                                                                                          |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------- |
| Canonical crate and CLI       | `zeroshot/`                                                                                                   |
| CLI grammar/help              | `zeroshot/src/native_v2_cli/parser.rs`                                                                        |
| CLI composition               | `zeroshot/src/native_v2_cli.rs`, `zeroshot/src/main.rs`                                                       |
| Built-in templates            | `zeroshot/src/native_v2_templates.rs`, `zeroshot/src/native_v2_templates/`                                    |
| Local run composition         | `zeroshot/src/native_v2_local.rs`                                                                             |
| Hosted/cloud composition      | `zeroshot/src/native_v2_cloud.rs`, `zeroshot/src/native_v2_hosting.rs`                                        |
| Hosted merge plans            | `crates/openengine-cluster-protocol/src/native_v2_hosted/merge_plan.rs`, `zeroshot/src/native_v2_cli/execution/merge_plans.rs`, `zeroshot/src/native_v2_target/controller_authority/hosted_runs/` |
| Portable controller           | `zeroshot/src/native_v2_portable_controller.rs`, `zeroshot/src/native_v2_portable_controller/`                |
| Provider/delivery composition | `zeroshot/src/native_v2_candidate.rs`, `zeroshot/src/native_v2_candidate/`                                    |
| Target server                 | `zeroshot/src/native_v2_target.rs`, `zeroshot/src/native_v2_target/`                                          |
| Target authority/auth         | `zeroshot/src/native_v2_target_authority.rs`, `zeroshot/src/native_v2_target_authority/`                      |
| Contained execution           | `zeroshot/src/execution.rs`, `zeroshot/src/execution/`                                                        |
| Faults and redaction          | `zeroshot/src/fault.rs`, `zeroshot/src/fault/`                                                                |
| Run ledger                    | `zeroshot/src/v2_run_ledger.rs`, `zeroshot/src/v2_run_ledger/`                                                |
| Cluster protocol types        | `crates/openengine-cluster-protocol/`                                                                         |
| Cluster server                | `crates/openengine-cluster-server/`                                                                           |
| Cluster client                | `crates/openengine-cluster-client/`                                                                           |
| Conformance fixtures          | `crates/openengine-cluster-testkit/`                                                                          |
| Worker descriptors/registry   | `crates/openengine-cluster-protocol/src/worker.rs`, `crates/openengine-cluster-server/src/worker_registry.rs` |
| Generated protocol artifacts  | `protocol/openengine-cluster/v1/`                                                                             |
| Documentation site            | `mkdocs.yml`, `docs/`, `scripts/docs_hook.py`, `.github/workflows/docs.yml`                                   |
| npm package                   | `npm/zeroshot/`                                                                                               |
| Target image                  | `docker/zeroshot-target/`                                                                                     |
| Target declarations           | `distribution/zeroshot-targets.json`                                                                          |
| Distribution tooling          | `scripts/distribution.js`, `scripts/distribution/`, `npm/zeroshot/lib/release-artifacts.js`                   |
| Python SDK                    | `sdks/python/`                                                                                                |
| Release workflow              | `.github/workflows/release.yml`                                                                               |
| Python release workflow       | `.github/workflows/release-python.yml`                                                                        |
| CI classifier                 | `.github/ci-path-classifier.js`                                                                               |
| Repository tooling tests      | `tests/tooling/`                                                                                              |

## Development conventions

- Fix root causes and keep changes scoped.
- Use existing patterns; do not add parallel registries, provider lists, model catalogs, or release
  authorities.
- Keep optional developer and agent analysis tools external to the repository. Do not add package
  dependencies, hooks, CI gates, skills, or checked-in state for personal analysis tooling.
- New Rust APIs must respect the four-parameter Clippy ceiling; use request structs rather than
  raising or bypassing the limit.
- Preserve bounded values, explicit overflow, cancellation safety, and exact source provenance at
  every public boundary.
- Add focused tests beside the owning crate/module.
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
pydoclint src/zeroshot
python -m mypy src examples
python -m pytest

cd ../..
python -m mkdocs build --strict
```

## Release convention

- CI has native, Python, and repository-tooling lanes plus stable aggregate `required`.
- `.github/workflows/release.yml` is the only canonical product release workflow.
- It publishes native archives/checksums, `ghcr.io/the-open-engine/zeroshot-target`, and
  `@the-open-engine-company/zeroshot`, then invokes Python revision `1`.
- Python revision `1` always produces its GitHub wheel release. PyPI publication is fail-closed by
  default and may be explicitly deferred with `publish_pypi: false`.
- `.github/workflows/release-python.yml` may publish later SDK-only revisions.
- `.github/workflows/docs.yml` publishes `main` as `dev` and canonical releases as immutable
  snapshots after Python revision `1`; later SDK-only releases do not rebuild product snapshots.
- There is no automatic semantic release, release-promotion branch, `dev -> main` flow, or second
  runtime release train.
