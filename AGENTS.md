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
- Pull request descriptions keep a nonempty `## Summary` with the user-facing change. Release notes
  come from that summary in the immutable squash commit, not from mutable GitHub metadata.
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
- `zeroshot update` resolves the newest canonical GitHub Release, verifies the declared host archive
  and canonical `zeroshot-skill.md` asset through that release's `SHA256SUMS`, smoke-checks the
  staged executable, and replaces the running executable in place without privilege escalation. It
  installs or refreshes only unchanged managed skill copies and preserves conflicts.
  Development-placeholder builds refuse self-update.
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
- Provider access defaults are materialized from the authored runtime at the execution-placement
  boundary. Omitted connections remain omitted in author-owned local profiles. Local Codex/OpenAI,
  Claude/Anthropic, and Copilot/GitHub lanes reuse native login state without inventing a
  connection requirement; contained placements and non-native lanes add only their canonical
  missing requirements. Explicit
  compatible authored fields always win. Remote profiles materialize contained requirements before
  storage, and the effective runtime drives the existing connection resolver and recovery contracts.
  The local CLI captures the invoking shell in its private one-shot bootstrap, consumes and deletes
  that file before controller effects, and keeps the detached controller's OS environment minimal.
  A later local CLI startup scavenges orphaned bootstrap files after the bounded controller handoff
  window without removing other run state.
  The in-memory snapshot supplies bounded native-context forwarding and Codex config-referenced
  provider variables; provider children still start from an empty environment. Snapshot values
  never enter profiles, ledgers, observation, or hosted adapters. Relative harness home overrides
  are resolved in the invoking process before detachment.
- Codex inherits harness settings for web search and sandbox network access. Runtime arguments
  select the admitted model, optional effort, and response contract; they must not override unrelated
  user preferences. Hosted workers and verifiers always use the harness's maximum approval/sandbox
  bypass inside their disposable capsule and do not query native permission policy. Local workers and
  verifiers use bypass only when the native configuration query proves no authored policy; configured
  or unavailable policy keeps native behavior. Explicit cached Codex web search also prevents local
  bypass because native full access can promote cached search to live. Configuration probes have a
  separate ten-second/4 MiB budget, never send a
  model prompt, suppress Claude hooks/auth helpers, and require confirmed process cleanup before the
  model turn. They do not rewrite settings files; normal native startup state may still be updated.
  Verifier nodes use the same permission handling as workers across all harnesses. The shared prompt
  renderer tells workers to use repository-declared setup in the checkout, await terminal command
  status, and keep standalone executable tools out of `/tmp`. Hosted verifiers receive private copies
  and may perform repository setup there. Local verifiers share the candidate and may run concurrently,
  so their prompt prohibits setup or dependency installs that rewrite it. Verifiers may run checks and
  create artifacts but cannot edit reviewed material or make repairs. A missing declared dependency
  requires setup and retry in a private copy, but rejects from a shared checkout rather than becoming
  an unavailable check. This local review boundary is instructional, not filesystem enforcement.
  Shared inspection owns bounded JSONL exchange and process cleanup. Each harness owns its native
  policy parser and `apply_permission_default` entry point; `PermissionPolicy` distinguishes unset,
  configured, and unavailable inspection results. Codex browser/computer access controls and explicit
  approval-review features count as authored policy.
- The local target registry initializes `cloud` at `https://api.cloud.zeroshot.sh` with a persistent hosted device identity.
- Named targets store only endpoint, access mode, and login identity. Named runs resolve repository, branch, exact remote revision, and worktree dirtiness client-side from the invoking Git worktree plus per-run overrides; target records never bind repositories.
- Hosted HTTP authority clones share one bounded in-memory OAuth access-token slot across all
  hosted operations, including merge-plan discovery routes. Reuse requires the same target, login,
  OAuth endpoints/client, and audience with more than thirty seconds of token lifetime remaining.
  Cache misses and login serialize under the shared cache lock before the cross-process refresh-family
  lock; tokens enter the cache only after session verification and rotated refresh-token persistence.
  Login clears the slot before attempting authentication. HTTP authentication rejection invalidates
  only the exact cached issuance, never a newer concurrent token; only merge plans retain their
  existing one-retry policy. Access tokens remain memory-only, and transport grant expiry continues
  to use durable observation reconnect/cursors without extending any lifetime.
- Direct-target submissions keep a separate secret-free local authorization for each run's connection field requirements. Resume must match target-reported requirements to that original authorization before reading the caller environment, constrain outgoing values to those fields, carry the authorization to the successor, and revoke it after workspace discard.
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
  revisits resume from the current user's `COPILOT_HOME` locally and a private home when contained.
  Local runs reuse stored Copilot login, host/auth endpoint settings, and supported custom-provider
  environment. Static custom-provider configuration and ambient or declared GitHub tokens cross
  private RPC only, never process/tool environments. Legacy command-backed keys cross the singular
  private provider contract so the pinned CLI refreshes them for each provider request. Local
  `providers.json` or `COPILOT_PROVIDERS_CONFIG` registries are read with size and entry bounds and
  transferred through protocol 3 because headless sessions do not import the path themselves; a
  selected registry provider with `apiKeyCommand` is translated to the working singular contract.
  Its helper receives eligible invoking-shell fields privately; runtime, authentication, provider,
  and parent-process loader controls are excluded, while every forwarded field is passed to
  Copilot's `--secret-env-vars`. This preserves per-request refresh without exposing those fields to
  shell or MCP tool environments. This matches native local Copilot's same-user trust boundary;
  secret-env filtering is not an OS identity boundary against adversarial same-UID process inspection.
  A nonempty registry takes precedence over legacy provider variables. Declared tokens suppress all
  ambient provider and offline controls. Declared endpoints cannot inherit ambient credentials or
  headers, and one declared credential form suppresses the other ambient forms. The admitted model
  is supplied unchanged to the headless process and normally to its RPC session while provider
  `modelId` capability and wire-model mappings remain intact. Translating a selected registry
  command uses that registry entry's authored `modelId` for the singular RPC session; the admitted
  `provider/id` remains the unchanged process-level registry selection. Contained runs require
  `COPILOT_GITHUB_TOKEN`. Optional
  `COPILOT_GITHUB_TOKEN_EXPIRES_AT` is Unix seconds;
  expiring credentials use the runtime resolver callback, validate the configured GitHub host, and
  must retain more than one hour. Present but empty or malformed declared credentials fail closed
  instead of falling back to native login.
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
- `openengine.workspace-checkpoints/v1` adds `run/checkpoints` and selected checkpoint resume to
  retained workspace recovery. Default resume starts a fresh graph on the latest retained workspace;
  checkpoint resume restores a private input snapshot and settled prerequisite executions. Neither
  imports old provider sessions, secrets, or token usage. Checkpoint IDs never become filesystem paths.
  The reducer owns boundaries: an outer parallel or mapped group is one unit across all nested work
  and dispatch waves. The supervisor captures only after all live executions and cleanup have settled;
  read-only boundaries reuse bytes. Local/direct storage normalizes Git into a temporary private
  filesystem stage, commits it to one deduplicated Restic repository per recovery lineage, and then
  removes the full stage. Small catalogs atomically map logical checkpoint IDs to Restic snapshots;
  versioned execution seeds live in separate files and are read only for selected continuation.
  Restart restores the latest published snapshot without importing a seed. Restore staging is a
  private sibling of the workspace so final replacement stays on one filesystem. Successful
  lineages delete their catalogs and repository only after durable terminal success; failed lineages
  retain both for resume.
  Hosted factories advertise checkpoints only when they implement storage and restore. Snapshot
  restore requires exclusive workspace ownership and preserves the checkout root and Git identity.
- Hosted workspace recovery advertises `resume`, `checkpoints`, and `discard_workspace` in the
  separate `openengine.hosted-workspace-recovery/v1` outer discovery extension, leaving the strict
  `zeroshot.hosted-runs/v1` route object unchanged for older clients. POST bodies and results match
  the OECP `Run*Params`/`Run*Result` types. These authenticated Cloud routes remain available after
  capsule disposal. Cloud resolves the admitted connection references freshly; hosted recovery does
  not depend on local direct-target authorization or read local provider environment values.
  `ProductionHostingConfig` can accept host-owned `HostedWorkspaceStorage`; the allocator invokes
  it after source checkout and before provider dispatch, imports settled prerequisites, and
  preserves supplied delivery lineage. Public clients never supply storage paths or execution seeds.
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
- Contained provider sessions clean up surviving descendants immediately after the main process exits,
  before draining inherited output streams. Buffered output still drains under any explicit command
  deadline and a ten-minute ceiling while observing cancellation; cleanup stays scoped to its session.
- Git delivery owns authenticated repository operations and observes the run PR and branch before
  staging. It pushes the captured candidate SHA, preserves published ancestry and local work, and
  recognizes confirmed remote success after a lost response. Authorized branch updates may advance
  the unchanged candidate directly; other integrated remote changes return `repair_required` so the
  authored graph decides what work follows. Agents receive local refs and conflicts without the
  delivery credential. Closed PRs, identity changes and lost published ancestry stop delivery.
- Hosted delivery Git runs as the pinned workspace writer UID/GID, matching source checkout, so
  fetched objects, commits, merges, and partial failures remain writable by subsequent repairs.
  Admission excludes overlapping writers and requires their process cleanup before delivery;
  concurrent hosted verifiers retain distinct identities. Delivery confirms UID-wide helper cleanup
  before success, repair, error, cancellation, or panic can release that identity; unconfirmed cleanup
  is fatal. A caught delivery panic settles as a node crash after confirmed cleanup, without exposing
  its payload. Allocation requires an idle writer domain before authenticated checkout and rechecks
  it before deleting the workspace or releasing its identity lease; failed cleanup retains both.
  Allocation retains cleanup authority before checkout starts, including through startup errors and
  cancellation. Controllers confirm destruction before recording an allocation failure as terminal.
  Local Git retains the caller identity.
- Before first publication, delivery captures and fetches the exact current target revision and
  merges it while preserving candidate history and dirty work. Any changed candidate returns through
  the authored repair/review loop before push. Delivery feedback distinguishes immutable
  `sourceRevision` provenance from the captured `reviewBaseRevision`; verifiers and repairs preserve
  upstream changes. Unconfirmed baselines after external branch updates are explicitly unavailable.
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
  merge queues/deferrals, and succeeds only after observing the exact merged result. Configured
  branch-protection contexts remain pending until they appear on the exact PR head, preventing a
  newly opened PR from looking ready before its required workflow registers. Missing or stale human
  review may satisfy PR readiness when aggregate ref-update policy positively requires approval,
  required checks have settled, and no observed conversation, linear-history, or signature blocker
  remains. Deployment requirements do not block PR handoff. Missing review-policy evidence stays
  pending, and this exception never grants merge authority. Outside merge
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
- GitHub API permission, schema, and policy failures are delivery execution errors, never candidate
  repair. Authentication rejection remains an explicit refusal. Review synchronization retries only
  explicitly transient failures and bounded visibility races; an ordinary statusless failure or HTTP
  403 is not presumed transient. Only typed CI/feedback/conflict outcomes, Git command failures, or
  verified repository/workspace reconciliation may request an agent repair.
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
- Current software-change templates author `push@1`, `pr@2`, and `merge@3` Git delivery workers.
  Push publishes only the exact managed branch revision. PR delivery stops only after required CI,
  freshness, and conflict checks pass; missing review approval is allowed and no merge is requested.
  PR and merge workers read all visible issue comments, review summaries, and inline comments behind
  one exact-head fence. A run checkpoint routes each new or edited item through the existing repair
  and verification loop once. `pullRequestFeedback: ignore` skips that read without weakening GitHub
  policy checks. Feedback is untrusted text and delivery credentials never enter the repair worker.
- The built-in `auto-research` graph runs exactly ten iterations. Explorer, synthesizer, and
  challenger scouts propose bounded directions; evidence, method, and progress judges return
  `adopt`, `record_only`, or `abort`. Graph guards route any abort, unanimous adoption, and the
  remaining record-only consensus to separate finalizers. A second one-item phase keeps judge controls
  bounded while an independent auditor recomputes consensus from the finalized evaluations and verifies
  the decision, hashes, and retained or restored filesystem before the iteration can continue. Unanimous
  adoption keeps workspace changes. Record-only retains a supported negative
  or inconclusive finding while restoring changes; any abort restores changes and records invalid,
  incomplete, or unsafe evidence. The charter separates non-negotiable
  invariants from optional progress measures. Once evidence proves the retained workspace violates
  an invariant, selectors prioritize repair and judges cannot reject a verified repair solely for
  missing an optional optimization threshold. Mutable state and summaries identify the retained
  workspace and its invariant status separately from the best supported historical findings; a
  result from a restored artifact is never presented as current. Bootstrap returns the bounded scout
  and judge role arrays plus one experiment work item. Graph guards require exactly one explorer,
  synthesizer, challenger, evidence, method, progress, and experiment activation before dependent work
  can run. A one-time read-only verifier preflight enforces those sets before the ten-iteration loop;
  missing, duplicate, extra, or failed entries cannot silently shrink or rewrite the campaign.
  Iteration role workers execute independently
  and keep the verifier's control-assignment space bounded; judge prompts do not receive peer reviews.
  Scout, selector, experiment, or judge execution failure
  finalizes an aborted iteration, restores when needed, and lets the bounded loop continue. Recorder
  and recovery failures stop the run before another iteration or checkpoint. Files under
  `.zeroshot/research` are the durable protocol: finalized iteration directories are append-only,
  while per-iteration directories under ignored `scratch/` hold reversible backups and in-progress
  handoffs. Agent workers never
  use Git. The template supports no delivery or `push@1`; for push, a read-only manifest worker
  derives checkpoint metadata from the finalized ledger and one graph-owned delivery node is
  revisited after every iteration. Delivery cannot overlap a writer. A checkpoint manifest failure,
  delivery repair request, receipt-auditor error, or rejected receipt stops the run; checkpoint
  delivery never invokes an unreviewed writer repair.

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
  Hosted candidate workspace roots are writer-owned `0700`; workers cannot traverse another run's
  candidate, and the supervisor-owned production ledger and existing SQLite sidecars are `0600`.
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
- The experimental local ACP endpoint owns one workspace lease and reusable runner for the outer
  ACP session. Each prompt remains a separate admitted run with its own controller lock, durable
  ledger, supervisor lifecycle, run ID, and freshly resolved Git source provenance. Owner-scoped
  node sessions use stable slots derived from sorted graph node names, survive clean turn settlement,
  and close before the ACP session releases its runtime directory or workspace lease. Workspace
  identity, workspace lease, controller lease, and an ACP-specific turn lease are monitored;
  loss terminalizes active work as `runtime_lost` and poisons the session. The local UI requires both
  per-run leases before treating an in-process ACP owner without a controller socket as live.
  Ordinary runs retain run-scoped session keys.

## CLI and target contracts

- CLI grammar/help comes from the derived Clap `Cli` tree and Rust doc comments.
- The foreground run command carries one Ctrl-C signal across preparation, submission, and
  observation. An interrupt before the backend submission future is first polled cancels without
  entering the backend. Once polled, that future is preserved through its receipt or error because
  it may cross an irreversible admission boundary before yielding. A successful receipt is emitted
  before detaching without opening observation. Ctrl-C never force-stops a run.
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

## Shared workspace UI

- `ui/` owns React/Vite; `zeroshot/src/workspace.rs` owns shared catalog, authoring and validation
  behind the `workspace` feature, without static assets. `profile_ui.rs` adapts these services to
  standalone HTTP, with browser lifecycle in `profile_ui/server.rs`. Build `ui/dist` before enabling
  Cargo's optional `ui` feature.
  Releases and target images embed it. Build, development, and feature checks: [ui/README.md](ui/README.md).
- `zeroshot ui` serves local CLI profiles and ledgers at `http://127.0.0.1:4173/ui/` by default;
  `--listen` accepts loopback only. `zeroshot ui --target NAME` keeps profiles and authoring local
  while its server discovers the target's `zeroshot.run-history/v1` bounded list/detail/page
  routes; target coordinates and hosted credentials never enter the browser. The browser continues
  to use only the local `/ui/api/runs` BFF. Hosted reads reuse the named target's OAuth authority
  and refresh-token custody. Direct `target serve` mounts `/ui/` on its existing listener, uses
  `--storage` for profiles/history, and advertises run history only with that UI mount. It
  initializes its single controller before UI reads. Private/hosted targets reject this standalone
  mount; their host may advertise the authenticated capability. Opening the UI never starts a run.
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
- `/ui/api/data` creates typed bindings/promotions; `/ui/api/authoring` creates ordinary
  completion/error nodes. Their existing `profile_ui/data.rs` and `profile_ui/outcomes.rs` helpers
  compile under `workspace`; both return drafts without saving or running. Required sources need native success proof; source selection and error
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
- Runtime schemas/workers come from native contracts in `profile_ui/catalog.rs`, compiled under
  `workspace`. Git delivery keeps its fixed contracts and explicit pull-request/merge modes. Model suggestions are non-authoritative;
  identifiers remain opaque. Missing harness/provider links to runtime settings. JSON stays lossless.
- `native_v2_observability::history` owns admitted definitions, bounded native pages and canonical
  control records without the `ui` feature. Its exported semantic validators are the single host
  boundary for definition identity/version, canonical contiguous cursors, page/control coherence,
  and the reserved `runtime_failed`/`runtime_lost` failure metadata; UI and Cloud readers must reuse
  them after wire decoding. Definitions contain bounded admission and terminal facts, never the
  accumulated execution snapshot; readers accept that legacy field only to strip it. Observer clones
  share its bounded projection cache.
  `profile_ui/runs.rs` supplies local filesystem/list/SSE adapters; observation never creates, repairs
  or recovers a ledger or controller. Preserve ordered execution cursors, explicit gaps and string
  u64 IDs. SSE resumes after `Last-Event-ID` and drains through the terminal cursor before closing.
- Private targets export capability-authenticated `POST /native-v2/history/definition` and
  `POST /native-v2/history/page` over their already-owned observer. Direct and hosted access modes
  cannot use these private exports; Cloud applies per-run authorization before using its capability.
  Request bodies carry run identity and cursor, keeping the target query surface unchanged.
- Status reads query only an already-owned controller. Unpersisted runtime failure stays separate
  from durable snapshots/events and marks history incomplete. Missing authority drains retained
  history before an explicit error. Never invent completion; a durable terminal record takes precedence.
  History observation availability is separate from terminal run status: an available runtime failure
  can be emitted at the same cursor while following continues toward the final durable record.
  Cloud may remain `collecting` through target/archive handoff; `finished` alone cannot end follow.
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
  `embed.html` mounts the same editor/viewer without standalone menus. Its version-1 bridge
  validates the parent window and exact origin, binds workspace authority, and correlates document
  generations and save snapshots. Host service URLs remain same-origin; optional CSRF configuration
  carries cookie/header names only. Cloud owns menus, authentication, profile CAS and live/archive
  adapters ([zero-cloud #301](https://github.com/the-open-engine/zero-cloud/issues/301)).
- Shared fetch-based SSE preserves HTTP and stream problem codes, accepted execution cursors,
  bounded frame buffering and owned reader cancellation. A host source change must preserve the
  viewer's history prefix and playback position; it never changes native graph projection.

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
- `.github/workflows/coverage.yml` measures the default workspace and UI-feature Rust test surfaces
  on native changes and publishes LCOV to Coveralls. Coverage is observational and stays outside the
  required CI aggregate.
- `.github/workflows/release.yml` is the only canonical product release workflow.
- It generates the GitHub Release body from the exact first-parent commits since the preceding
  canonical tag. Every released commit must retain a Conventional Commit squash title ending in
  `(#PR)` and a nonempty release summary. Generation uses the exact release source, and publication
  persists the Markdown as an immutable release asset for byte-exact recovery.
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
