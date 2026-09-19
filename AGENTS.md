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
- Node.js is used for repository tooling, the npm binary delivery package, and building the static
  profile UI. The UI is served by Rust and never needs a production Node server.
- Canonical releases are explicit `vX.Y.Z` tags with major version 8 or newer.
- The npm package is `@the-open-engine-company/zeroshot`.
- That package owns one canonical skill and installs managed copies for Codex, GitHub Copilot, and
  Claude Code at their user scopes. Do not fork the skill by host.
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
- `zeroshot update` resolves the newest canonical GitHub Release, selects the declared host archive,
  verifies it through that release's `SHA256SUMS`, smoke-checks the staged executable, and replaces
  the running executable in place without privilege escalation. Development-placeholder builds
  refuse self-update.
- Release recovery may complete missing outputs only when existing immutable artifacts match the
  exact version and source commit.

## Runtime invariants

- Protocol Rust types are the source of truth. Generated files under
  `protocol/openengine-cluster/v1/` must be regenerated through the Rust testkit, not hand-edited.
- Initial input remains caller-owned and is validated unchanged. A root group may add required
  state fields only when their payload types have deterministic implicit empty values; verification
  proves this and reduction materializes missing null, string, and recursively empty record values.
- Map reduction collects a promoted field only when every item wrote it in that item scope. A worker
  error leaves an incomplete collection unchanged while its controls remain available to authored
  guards; never substitute inherited arrays, null placeholders, or shorter partial collections.
- Model identifiers are opaque provider-owned strings. Do not infer a harness from a provider/model,
  maintain runtime model catalogs, or validate provider availability. Admission may reject only known
  incompatible harness/provider pairs.
- Runtime selection requires caller-authored `harness`, `provider`, and `model` values.
- The `gateway` provider resolves `GATEWAY_BASE_URL` and `GATEWAY_API_KEY` through named
  connections. Codex uses Responses with bearer authentication; Claude uses Messages with
  `x-api-key`. Preserve caller-owned base paths and model identifiers without protocol detection.
- Local Codex `openai` runs inherit the user's configured model provider and transport, including
  OpenAI-compatible proxies. Preserve declared `OPENAI_API_KEY` for custom provider authentication
  while supplying the native `CODEX_API_KEY` alias. Hosted runs and explicit OpenRouter/Bedrock/gateway
  selections retain adapter-owned provider setup.
- Local runs preserve shell endpoint settings, Claude configuration directories and permission controls;
  declared connection values take precedence. Hosted adapters do not inherit ambient settings.
  Endpoint and Claude control variables in declared connections are passed to the harness; active
  transport selectors cannot contradict an explicitly selected OpenRouter, Bedrock, or gateway lane.
- Codex inherits harness settings for web search and sandbox network access. Runtime arguments
  select the admitted model, optional effort, and response contract; they must not override unrelated
  user preferences. Local and hosted workers and verifiers use approval/sandbox bypass only when the
  harness's native configuration query proves no authored permission policy. Configured or unavailable policy
  keeps native behavior. Explicit cached Codex web search also prevents bypass because native full
  access can promote cached search to live. Configuration probes have a separate ten-second/4 MiB budget, never send a
  model prompt, suppress Claude hooks/auth helpers, and require confirmed process cleanup before the
  model turn. They do not rewrite settings files; normal native startup state may still be updated.
  Verifier nodes use the same permission handling as workers across all harnesses. The shared prompt
  renderer always adds verifier guidance prohibiting edits to material under review and repairs while
  allowing checks and their generated artifacts. Local verifiers share the candidate; hosted verifiers
  retain private copies. This local review boundary is instructional, not filesystem enforcement.
  Shared inspection owns bounded JSONL exchange and process cleanup. Each harness owns its native
  policy parser and `apply_permission_default` entry point; `PermissionPolicy` distinguishes unset,
  configured, and unavailable inspection results. Codex browser/computer access controls and explicit
  approval-review features count as authored policy.
- The local target registry initializes `cloud` at `https://api.cloud.zeroshot.sh` with a persistent hosted device identity.
- Named targets store only endpoint, access mode, and login identity. Named runs resolve repository, branch, exact remote revision, and worktree dirtiness client-side from the invoking Git worktree plus per-run overrides; target records never bind repositories.
- Portable worker bindings resolve through the generic `WorkerRegistry` boundary. External binding
  protocol, version, and profile values are bounded opaque strings; the protocol crate must not
  keep an external binding catalog. `openengine.worker.builtin/v1` is reserved for native
  in-process workers. No portable external binding currently ships; do not reintroduce retired
  worker profiles.
- Structured-output correction runs in the same provider session under the node's existing
  execution policy; local response validation remains authoritative.
- Provider continuation is bounded: Claude continues once after `system/api_retry`; Codex continues
  once after a terminal execution error. Both send literal `Continue` in the same session when one
  exists. Structured output receives at most two correction turns before `malformed`.
- Copilot uses the pinned CLI's headless JSON-RPC protocol 3, with provider `github` and
  caller-owned model IDs. Structured output and corrections share one session; node-instance
  revisits resume it from the private home. `COPILOT_GITHUB_TOKEN` crosses private RPC only,
  never process/tool environments. Optional `COPILOT_GITHUB_TOKEN_EXPIRES_AT` is Unix seconds;
  expiring credentials use the runtime resolver callback and must retain more than one hour.
  Copilot RPC bounds each message to 64 MiB and bounds pending requests and output queues.
- Provider JSONL readers do not cap cumulative output. They share only the 64 MiB unfinished-record
  guard, accept a complete final record without a newline, ignore unknown future event types before
  validating provider-owned fields, and continue draining after the first valid terminal event.
- Provider stdin and stdout are concurrent and bounded so large prompts and early output cannot
  deadlock. Incomplete stdin is fatal, while parsed identity, usage, retry, and diagnostics survive
  either I/O completion order.
- Durable provider events cross bounded async queues with backpressure. Cancellation preserves token
  usage and event order; overflow is explicit and produces an incomplete marker rather than silent
  loss.
- SQLite operations acquire the single connection asynchronously before running on a blocking
  thread; cancelling a caller does not release an in-flight database operation's ownership.
- Live force-stop signals owned work before waiting for persistence. Local stop intent prevents
  interrupted work from becoming a retryable crash. Cleanup and durable output still precede
  final settlement. Confirmed runtime failure is observable before its persistence attempt;
  a later durable terminal snapshot remains authoritative over the in-memory fallback.
- Failed local and direct-target runs retain an exclusively claimable workspace for a successor
  attempt. Local recovery metadata lives beside each run ledger while the checkout remains
  user-owned; direct targets retain only the candidate and Git metadata, dispose private runtime
  state after confirmed process cleanup, quarantine retained trees under supervisor ownership, and
  recursively transfer them to the successor's newly leased writer identity before admission. They
  advertise `openengine.workspace-recovery/v1`. Local resume persists predecessor/successor
  lineage before controller launch, uses a process-held file lock during admission, and reconciles
  an interrupted launch before permitting another successor. Direct-target retained-workspace
  handoff records both sides before moving the tree and reconciles incomplete handoffs at startup.
  Direct-target status exposes the immutable admitted connection requirements so the CLI resolves
  fresh resume values without consulting changed profiles. Recovery lineage also retains the root
  attempt's delivery identity so every successor reuses the same delivery branch and pull request.
  Successor composition explicitly authorizes a fresh delivery adapter to adopt that lineage-owned
  branch; ordinary fresh adapters still reject unexplained existing run branches.
- Durable observation replay reads bounded ledger pages and retains its scan cursor across pages.
  A finished snapshot closes a subscription only after replay reaches its durable cursor.
- Bulk replay uses the WebSocket client's opt-in subscription backpressure on a dedicated
  connection; control requests and ordinary observers keep their independent connections.
- Safe-log timestamps are captured at the producer boundary as positive JavaScript-safe Unix epoch
  milliseconds and remain unchanged across durable replay.
- Runner start reserves an execution and returns its handle before asynchronous provider startup.
  Pending start waits are cancellable; accepted startup failures settle through the owned handle.
  Cancellation drains output and provider cleanup before execution settlement. Run close reserves
  and tombstones execution activity atomically; no late work may surface after close returns.
- Node deadlines are optional: omitted `timeoutMs` means completion or explicit cancellation.
  Built-in graphs have no node deadlines, and provider adapters impose no separate turn timeout.
  The supervisor records node error codes and elapsed time in durable logs before settlement.
- A failed durable-output bridge cancels and drains its provider immediately. Fatal supervisor
  errors and task panics close owned work and attempt runtime cleanup before durable failure.
  If persistence is unavailable, the controller retains a minimal `runtime_failed` status at the
  last observed durable cursor and records private operator diagnostics, including SQLite error codes.
  This fallback creates no history events. Readable retained history drains normally; unavailable
  history closes with `SOURCE_UNAVAILABLE`, never `done`. Compiler/runtime failure reasons
  `unhandled`, `runtime_failed`, and `runtime_lost` are reserved against authored graph fail nodes.
- Native-v2 retries only a settled `crash` outcome when the executable has another authored
  attempt. A provider session invalidated by that active execution becomes replaceable for the
  authorized retry; passive session loss and run closure remain permanent, fail-closed loss.
- Hosted source checkout retries only its fresh platform-owned staging workspace, within one
  allocation and one total deadline. Preserve the exact admitted revision before starting any
  graph node; terminal Git details remain redacted and private operator diagnostics.
- Contained provider sessions bound post-exit I/O draining by any explicit command deadline and a ten-minute
  ceiling while still observing cancellation and cleanup.
- Git delivery owns authenticated repository operations and observes the run PR and branch before
  staging. It pushes the captured candidate SHA, preserves published ancestry and local work, and
  recognizes confirmed remote success after a lost response. Authorized branch updates may advance
  the unchanged candidate directly; other integrated remote changes return `repair_required` so the
  authored graph decides what work follows. Agents receive local refs and conflicts without the
  delivery credential. Closed PRs, identity changes and lost published ancestry stop delivery.
- Delivery retries recognized transport failures within the caller's polling and cancellation
  policy, refreshing dynamic credentials once after authentication failure. Temporary credential
  resolution retries with backoff; confirmed refusal and malformed responses stop. Initial
  resolution belongs to each supervised execution, so it observes the authored deadline and does
  not block parallel dispatch. Pending authorized head adoption survives repair.
- Git diagnostics retain bounded, credential-redacted command/status/stdout/stderr details;
  unfamiliar failures use the existing optional `repair_required` signal. Only successful receipts
  require complete remote identity. Live output splits large UTF-8 diagnostics into bounded records.
  Delivery checks unresolved index entries before `git add --all` and completes resolved merges even
  when their tree has no staged difference. Other unfinished Git operations return raw status for
  repair before staging or reconciliation. Delivery does not inspect or filter user-installed tooling.
- GitHub delivery treats aggregate merge policy and required contexts as authority, waits through
  merge queues/deferrals, and succeeds only after observing the exact merged result. Outside merge
  queues, branch freshness advances only through an authorized compare-and-swap response. A
  reported conflict is routable only after the trusted lane fetches the exact current target and
  leaves a verified nonempty Git merge conflict in the workspace; repair agents receive no GitHub
  credential, and the trusted lane pins their repository-local commit identity to
  `Zeroshot <delivery@zeroshot.invalid>` before handoff. Merge receipts preserve GitHub's
  authoritative merged revision. When a required check fails, bounded repair diagnostics include
  supporting failed checks and labelled raw job excerpts ending at the last GitHub error annotation.
  Supporting checks never acquire merge authority, and unavailable or omitted logs are explicit.
  Job-log reads opt into raw terminal sequences only inside bounded pipe capture, then remove
  controls before feedback; older GitHub CLI versions retry without the unsupported opt-in flag.
- Delivery-enabled software-change templates make the acceptance verifier the sole author of the
  current change title and description after every review pass. Git delivery uses that manifest for
  commits and reviews, refreshes only its marker-delimited body section while preserving surrounding
  text. Verifier guidance reserves closing references for delivery, which appends them only from
  caller-owned issue input.
- GitHub review creation and rediscovery are shared by pull-request and merge delivery. They verify
  the exact pushed ref and head, retry bounded transient visibility or API failures, refresh a
  dynamic credential once on HTTP 401 within the synchronization deadline and cancellation
  boundary, and fail closed on identity mismatch or static-token rejection. Verifier-authored pull
  request descriptions and source-issue closing references stay inside the generated body markers
  so refreshing metadata cannot retain a stale issue reference. Reviews with an unowned closing
  reference in a legacy Zeroshot layout fail closed instead of rewriting ambiguous human text.

- Target images apply current Debian Trixie package updates and install a checksum-verified upstream
  GitHub CLI. Image tests exercise GraphQL pagination with the installed CLI before publication.
  They ship one Rust toolchain baseline plus Node.js, Python and shared native build tools. They expose
  Rust through the fixed runtime PATH without a shared writable Cargo cache;
  explicit user toolchain settings and installations take precedence. Runtime toolchain smoke
  tests compile native fixtures as an isolated user with a read-only root and fresh home.
- Native-v2 admits concurrent writers in parallel branches and map items. Writers share the run's
  workspace owner identity; hosted session cleanup tracks an immutable supplementary group marker
  per session. Authored graphs coordinate overlapping edits. Admission rejects Git delivery that
  can overlap another writer or delivery; delivery may run alongside verifiers, which are instructed
  not to edit the candidate.
  A delivery receipt certifies success only if every other writer settled before delivery started.
  Unconfirmed process cleanup is a fatal runtime failure, including after cancellation; it cannot
  be reduced to a retryable node crash or an authored parallel-join void.
  Retained workspace handoff commits when the source workspace moves to its successor. A later
  allocation failure preserves that successor for recovery; unconfirmed cleanup keeps its durable
  run nonterminal until replacement-controller reconciliation confirms cleanup.
- Hosted verifiers build in disposable writable copies of the current candidate. Copies include
  dirty files and build artifacts, preserve metadata, and use reflinks or independent file copies.
  Source traversal pins descriptors without following symlinks so concurrent renames cannot escape
  the candidate; copying alongside writers does not provide an atomic snapshot. Verifier writes
  are never promoted to the candidate or peers. Provider scratch permits execution.
  Managed copies and execution-scoped homes are removed only after confirmed process-tree cleanup;
  node-instance homes survive authorized continuation and loop revisits until session closure.

- Native local CLI and in-process execution support Unix and Windows. Shared OS facilities live in
  `execution::platform`; local controller transport selects Unix sockets or private Windows named
  pipes behind one NDJSON protocol. Windows state uses protected current-user/SYSTEM ACLs, rejects
  reparse points, and pins volume/file identity. Provider and delivery descendants belong to
  kill-on-close Job Objects before their first instruction. Detached controllers inherit no caller
  handles and resume only after proving they escaped every caller Job; restrictive Job policies
  reject controller startup. Windows config defaults to
  `%LOCALAPPDATA%/zeroshot` and state to its `state` directory. Hosted target isolation remains Linux-only.

## CLI and target contracts

- CLI grammar/help comes from the derived Clap `Cli` tree and Rust doc comments.
- Graph verification errors display their first safe diagnostic through the shared verifier error,
  so local validation and hosted rejection report the same cause.
- Do not hand-edit `docs/zeroshot-cli.md` or `docs/zeroshot-cli.html`; regenerate with
  `cargo run -p zeroshot --example generate_cli_docs -- --write` and verify with `--check`.
- The public documentation site is the root `mkdocs.yml`. Python API pages are generated from the
  curated SDK exports and docstrings. Do not restore a second SDK-only MkDocs site.
- Keep `docs/reference/python/*.md` out of Prettier; its Markdown formatter removes the indentation
  required by mkdocstrings directives. Rendered-symbol CI checks are the contract.
- `docs/reference/cluster/api.md` is generated from the final OpenRPC value
  through the Rust testkit. Do not hand-edit it or add a parallel method registry.
- Published documentation defaults to `current/` from `main` and keeps one moving `vX.Y/` version
  per minor release. Each minor advances to its newest published patch; retries cannot roll it back
  or substitute another source for the same product version. Schema-2 manifests record exact product,
  SDK, source, and publication-tooling identity. Legacy patch page URLs redirect to their minor.
- Documentation publication tools come from the workflow commit separately from the exact release
  source. The publisher migrates the existing Pages tree locally and pushes once after validation.
  A first release publication bootstraps Current from `main` in an isolated checkout before setting
  the default; Current never inherits the requested release's product identity.
- The direct target's discovery, sourceful run request, and run-scoped OECP session are versioned
  native-v2 protocol contracts. Do not add alternate endpoints as aliases.
- Secret-bearing target inputs never enter run ledgers, target configuration, or observation records.
- Target transport failures retain the target name, origin and safe connection category. Never expose
  raw request URLs or credentials; transport failures remain eligible for observation reconnection.
- Target HTTP failures use the shared bounded `{code,message,details?}` protocol problem; message-only
  bodies are invalid, and details contain only user-safe structured metadata.
- Operator diagnostics are private-capability-only, run-scoped, bounded, sanitized, and excluded
  from public run status and logs.
- Hosted merge plans are atomic, immutable, merge-only DAGs over one explicit repository, branch,
  and profile. The target resolves each node's exact revision only after its dependencies succeed;
  plans have static inputs, no cross-node dataflow, and no retry-in-place. Agent runtime bindings
  cannot declare `GH_TOKEN`; the sole merge-delivery binding owns the GitHub write credential. A
  node's queue deadline is the earlier of plan expiry and seven days after readiness.
- Read-only safe commands include `zeroshot list`, `zeroshot status`, and `zeroshot logs`.
- Destructive commands such as `zeroshot force-stop` require explicit user intent.

## Where to look

| Concept                       | Path                                                                                                                                                                                              |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Canonical crate and CLI       | `zeroshot/`                                                                                                                                                                                       |
| CLI grammar/help              | `zeroshot/src/native_v2_cli/parser.rs`                                                                                                                                                            |
| CLI composition               | `zeroshot/src/native_v2_cli.rs`, `zeroshot/src/main.rs`                                                                                                                                           |
| CLI self-update               | `zeroshot/src/native_v2_cli/update.rs`                                                                                                                                                            |
| Built-in templates            | `zeroshot/src/native_v2_templates.rs`, `zeroshot/src/native_v2_templates/`                                                                                                                        |
| Local run composition         | `zeroshot/src/native_v2_local.rs`                                                                                                                                                                 |
| Hosted/cloud composition      | `zeroshot/src/native_v2_cloud.rs`, `zeroshot/src/native_v2_hosting.rs`                                                                                                                            |
| Hosted merge plans            | `crates/openengine-cluster-protocol/src/native_v2_hosted/merge_plan.rs`, `zeroshot/src/native_v2_cli/execution/merge_plans.rs`, `zeroshot/src/native_v2_target/controller_authority/hosted_runs/` |
| Portable controller           | `zeroshot/src/native_v2_portable_controller.rs`, `zeroshot/src/native_v2_portable_controller/`                                                                                                    |
| Provider/delivery composition | `zeroshot/src/native_v2_candidate.rs`, `zeroshot/src/native_v2_candidate/`                                                                                                                        |
| Target server                 | `zeroshot/src/native_v2_target.rs`, `zeroshot/src/native_v2_target/`                                                                                                                              |
| Target authority/auth         | `zeroshot/src/native_v2_target_authority.rs`, `zeroshot/src/native_v2_target_authority/`                                                                                                          |
| Contained execution           | `zeroshot/src/execution.rs`, `zeroshot/src/execution/`                                                                                                                                            |
| Faults and redaction          | `zeroshot/src/fault.rs`, `zeroshot/src/fault/`                                                                                                                                                    |
| Run ledger                    | `zeroshot/src/v2_run_ledger.rs`, `zeroshot/src/v2_run_ledger/`                                                                                                                                    |
| Cluster protocol types        | `crates/openengine-cluster-protocol/`                                                                                                                                                             |
| Cluster server                | `crates/openengine-cluster-server/`                                                                                                                                                               |
| Cluster client                | `crates/openengine-cluster-client/`                                                                                                                                                               |
| Conformance fixtures          | `crates/openengine-cluster-testkit/`                                                                                                                                                              |
| Worker descriptors/registry   | `crates/openengine-cluster-protocol/src/worker.rs`, `crates/openengine-cluster-server/src/worker_registry.rs`                                                                                     |
| Generated protocol artifacts  | `protocol/openengine-cluster/v1/`                                                                                                                                                                 |
| Documentation site            | `mkdocs.yml`, `docs/`, `scripts/docs_hook.py`, `scripts/docs_versions.py`, `.github/workflows/docs.yml`                                                                                           |
| npm package                   | `npm/zeroshot/`                                                                                                                                                                                   |
| Target image                  | `docker/zeroshot-target/`                                                                                                                                                                         |
| Target declarations           | `distribution/zeroshot-targets.json`                                                                                                                                                              |
| Distribution tooling          | `scripts/distribution.js`, `scripts/distribution/`, `npm/zeroshot/lib/release-artifacts.js`                                                                                                       |
| Python SDK                    | `sdks/python/`                                                                                                                                                                                    |
| Release workflow              | `.github/workflows/release.yml`                                                                                                                                                                   |
| Python release workflow       | `.github/workflows/release-python.yml`                                                                                                                                                            |
| CI classifier                 | `.github/ci-path-classifier.js`                                                                                                                                                                   |
| Repository tooling tests      | `tests/tooling/`                                                                                                                                                                                  |

## Standalone workspace UI

- `ui/` owns React/Vite; `zeroshot/src/profile_ui.rs` owns native services, with browser lifecycle
  in `profile_ui/server.rs`. Build `ui/dist` before enabling Cargo's optional `ui` feature.
  Releases and target images embed it. Build, development, and feature checks: [ui/README.md](ui/README.md).
- `zeroshot ui` serves local CLI profiles and ledgers at `http://127.0.0.1:4173/ui/` by default;
  `--listen` accepts loopback only. Direct `target serve` mounts `/ui/` on its existing listener,
  uses `--storage` for profiles/history, and initializes its single controller before UI reads.
  Private/hosted targets reject this standalone mount. Opening the UI never starts a run.
- Browser access requires the configured public origin, exact Host, and valid Fetch-Site;
  forwarded headers cannot broaden authority. UI keepalive requests cannot reach target control
  endpoints. Keep JSON writes, request bounds, and connection-owned SSE readers/timers.
- SIGINT/SIGTERM close listeners and observers with bounded draining. Stopping the local UI leaves
  detached runs active. Target restart reconciles interrupted runs as runtime loss, never as an
  invented user stop or automatic retry.
- `WorkspaceServices` separates profile storage, native authoring, run listing, and observation.
  `RunHistorySource` supplies the viewer's run identity and transport. The standalone shell owns
  selection/navigation; graph components must remain independent of the host.
- Profiles use `LocalRunProfileStore` and `NativeV2Admission::validate_profile`. Saves check content
  revision and the bootstrap workspace UUID under the CLI's store lock. Identity persists with the
  store; replacement rejects stale tabs, including creation requests without a prior revision.
- Drafts/layout are scoped to workspace identity. Saves acknowledge only their captured document
  generation; imports carry no foreign revision. Pending numeric text belongs to the document,
  survives inspector navigation, and commits only when valid. Navigation must preserve unsaved work.
- The structured graph is canonical; React Flow/ELK provide presentation only. Flattening sequences
  or folding standard error routes must preserve scopes, identities, priority, conditions, and
  runtime bindings. Custom/unfamiliar routes stay visible. Layout and themes stay out of profiles.
- Normal authoring exposes Inputs and Outputs. Run inputs/results and runtime settings belong in
  the toolbar. Do not restore state, mapping, promotion, join-strategy, or error-policy panels.
  Keep defaults/restriction prose on the info icon's `#defaults` page, preserving the mounted editor.
- `/ui/api/data` (`profile_ui/data.rs`) creates typed bindings/promotions; `/ui/api/authoring`
  (`profile_ui/outcomes.rs`) creates ordinary completion/error nodes. Both return drafts without
  saving or running. Required sources need native success proof; source selection and error
  protection form one transaction, preserving custom recovery, reasons, priority, and terminal scope.
- Optional promotions preserve absence; required reads still need availability proof. Promote only
  current-scope writes, never stale results from a prior loop iteration. Required incoming state
  retains carry semantics. Maps collect in input order, including `[]`; never infer a branch winner.
- Choice outputs require a producer on every continuing alternative, with first-match execution
  and success proof. Later writes invalidate pending presence facts. Loop error exits require a
  guaranteed completing Step; fixed loops complete on normal exhaustion, review loops fail.
- Data edits update only owned routes and linked schemas, including carried state and Map items.
  Removing a producer leaves typed, unbound inputs. Previous-attempt reuse requires native proof
  of the original source and compatible path. Label only unchanged caller values as Run input.
- Structural actions are atomic and preserve identities, bindings, and completion checkpoints.
  Parallel grouping checks dependencies; body replacement removes obsolete bindings. Converting a
  Verifier rejects remaining verifier-only contracts. Review loops carry feedback into the next
  attempt, starting with an implicit empty string; reviewers never inherit credentials or sessions.
- Artifact-producing work uses writing Agents; Verifiers perform independent checks/classification.
  Parallel/Map writers remain writers. Examples pass file paths and compact decisions, with separate
  files per writer and fresh directories per mapped item. Authors coordinate overlapping edits.
- Runtime schemas/workers come from `profile_ui/catalog.rs` native contracts. Git delivery keeps its
  fixed contracts and explicit pull-request/merge modes. Model suggestions are non-authoritative;
  identifiers remain opaque. Missing harness/provider links to runtime settings. JSON stays lossless.
- `profile_ui/runs.rs` reads ledgers without creating, repairing, or recovering them. Definitions
  come from admission snapshots. Preserve ordered cursors, explicit gaps, bounded pages, and string
  u64 IDs. SSE resumes after `Last-Event-ID` and drains through the terminal cursor before closing.
- Status reads query only an already-owned controller. Unpersisted runtime failure stays separate
  from durable snapshots/events and marks history incomplete. Missing authority drains retained
  history before an explicit error. Never invent completion; a durable terminal record takes precedence.
- `run-history.ts` projects only the selected history prefix. Rust's canonical reducer supplies
  structural visits/decisions with source cursors and stable identities; JavaScript never evaluates
  guards or invents worker executions. Keep loop visits, retries, and Map items distinct. Projection
  failure leaves recorded worker history readable; verifier rejection is a decision, not a crash.
- Read-only history reuses `WorkflowCanvas`; groups start collapsed and scrubbing preserves expansion.
  Seek pauses following; transcript scrolling stays live and follows only at the bottom. Keep the
  execution pinned while reading. Node timelines use the shared cursor; transcripts remain bounded.
  Initial/live batches stop at 5,000 events or 8 MiB until explicit continuation. Simulated histories
  remain separate from real runs and profile storage; scenarios/evidence live under `ui/qa/`.
- Visual tokens/fonts follow `zero-cloud/frontend/VISUAL_DESIGN.md`; bundle all runtime assets.
  Cloud embedding is pending [zero-cloud #301](https://github.com/the-open-engine/zero-cloud/issues/301):
  Zeroshot still needs a shell-less entry, host bridge, authenticated definition/history export,
  and structured stream errors. Cloud owns menus, auth, profile CAS, and live/archive adapters.

## Development conventions

- Fix root causes and keep changes scoped.
- Use existing patterns; do not add parallel registries, provider lists, runtime model catalogs, or release
  authorities.
- Keep optional developer and agent analysis tools external to the repository. The npm-delivered
  Zeroshot product skill is the sole product-owned exception; do not add package dependencies,
  hooks, CI gates, other skills, or checked-in state for personal analysis tooling.
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
cargo test --workspace # Unix; Windows: powershell -NoProfile -File scripts/test-windows.ps1
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

- CI has native, Python, repository-tooling, npm-package, and strict-documentation lanes plus stable
  aggregate `required`. `.github/ci-path-classifier.js` owns fail-closed path routing and cross-lane
  producer/consumer dependencies. The native lane runs on Linux and Windows, including real local
  CLI subprocess tests. Windows uses
  `scripts/test-windows.ps1` to run test executables outside Cargo's restrictive Job; the CI-only
  `.github/scripts/test-windows-host.ps1` also starts outside the hosted runner's Job. Linux also executes
  hosted process and filesystem boundary tests as root against its built test binary.
- `.github/workflows/release.yml` is the only canonical product release workflow.
- It publishes native archives/checksums, `ghcr.io/the-open-engine/zeroshot-target`, and
  `@the-open-engine-company/zeroshot`, then invokes Python revision `1`.
- Python revision `1` always produces its GitHub wheel release. PyPI publication is fail-closed by
  default and may be explicitly deferred with `publish_pypi: false`.
- `.github/workflows/release-python.yml` may publish later SDK-only revisions.
- `.github/workflows/docs.yml` publishes `main` as Current and canonical releases into `vX.Y/`
  after Python revision `1`; later SDK-only releases do not rebuild product documentation.
  The root always selects Current. Legacy `dev` and `stable` redirects stay outside the selector.
- There is no automatic semantic release, release-promotion branch, `dev -> main` flow, or second
  runtime release train.
