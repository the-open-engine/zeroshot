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
- That package owns the canonical `zeroshot`, `zeroshot-memory-init`, and `zeroshot-graph-design`
  skills and installs managed copies for Codex, GitHub Copilot, Pi, and Claude Code at their user
  scopes. Do not fork skills by host.
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
  and canonical skill assets through that release's `SHA256SUMS`, smoke-checks the
  staged executable, and replaces the running executable in place without privilege escalation. It
  installs or refreshes only unchanged managed skill copies and preserves conflicts.
  Development-placeholder builds refuse self-update.
- Release recovery may complete missing outputs only when existing immutable artifacts match the
  exact version and source commit.

## Runtime invariants

- Active Memory is host-owned opt-out state, separate from the checkout. Local CLI runs use one
  database under local state; direct targets use one persistent target-wide database with repository
  scopes. Generic hosting embedders explicitly select a namespace; Platform and ACP integration are
  not enabled implicitly. Runtime `activeMemory` independently controls repository and graph
  collection/recall; `globalAgents.postmortem` controls the built-in after-run agent and inherits an
  existing agent node's provider/model/connections. Four isolated verifier specialists separately maintain
  graph lessons, maintain repository lessons, discover graph procedures, and discover repository
  lessons. Maintenance contexts and mutation authority are partitioned by lesson scope, including
  repository-qualified graph lessons; each scope's opt-out disables its maintenance and discovery.
  Adaptation provenance names the responsible scoped maintenance specialist. Maintenance uses
  complete byte-bounded batches and requires exactly one decision per supplied ID, bound by the host to its frozen revision; omitted
  or duplicate decisions cannot count as completed reconciliation. The host owns revision preconditions
  and increments; agents do not echo or propose revision counters, while prior initialization responses
  with an explicit revision remain accepted only when it matches the frozen supplied version. Maintenance consolidates redundant
  paraphrases only within the same repository or exact graph/node/repository-qualified scope, retaining
  one canonical recommendation and all useful detail while deciding every supplied ID. Rejected discovery proposals report a validation stage and host-owned card field names without echoing
  proposal text. Repository advice admission detects common literal file:line positions for associated/cited source
  while preserving ambiguous endpoint, URL and image references;
  legacy cards remain readable but scoped maintenance must adapt or defer them rather than keep
  or renumber volatile positions. Postmortem transcript collection scans bounded ledger pages and reclaims its fixed
  byte budget from the largest execution tail, preserving earlier worker evidence under late reviewer
  output rather than using a global last-record window. Replay observes cancellation between bounded
  reads; final context trimming shrinks the largest arrays and retains per-execution transcript tails. Discovery has no findings count
  quota: impact, recurrence, current evidence and usefulness determine admission. Shared transactions
  are bounded by serialized bytes and store capacity rather than a mutation count quota. Their work runs
  before runner closure/cleanup, preserves the durably latched primary outcome and its failure
  diagnostics on ordinary failure or controller restart after confirmed cleanup, and
  stays outside reducer/checkpoint history. All usage remains part of the run. Never turn incomplete
  supplied-memory evidence into an empty manifest. Graph guidance supplements only private provider
  invocations after plan validation; it never mutates authored instructions or graph hashes. Hooks
  retrieve complete cards from a frozen resident index with relevance ranking and a cumulative
  session byte budget. Execution-scoped conversations receive fresh recall budgets; reused
  node-instance conversations retain theirs. Windows memory storage and initialization use shared
  private-state helpers, including validation of SQLite sidecars. SQLite-created Windows children
  and externally written initialization responses may inherit the token's default owner only with
  an already private ACL and verified private parent; normalize them to explicit current-user
  ownership before use. Initialization creates temporary files with private security at
  creation rather than repairing default-owned files afterward.
  Shared store mutations use expected revisions and retained deletion tombstones.
  Codex trusts only the exact normalized hash of the product-owned session hook; never bypass trust
  for unrelated native hooks or write user trust configuration. Callback receipts acknowledge returned
  context, not model compliance. Missing carrier acknowledgement, failed provider startup or bounded
  transport failure makes maintenance evidence unknown. Callback frames have absolute I/O deadlines
  so a stalled child cannot keep the resident broker or run cleanup alive indefinitely.
  Interrupted auxiliary work marks run usage incomplete. New lessons require observed costly main
  work; a routine response-format correction alone cannot qualify. Bounded in-memory native tool
  receipts support this check across all harnesses without retaining arguments or tool results.
  Review-cost proof counts distinct complete rejected rounds in one map scope and attributes them
  to participating serial-prefix workers. Supported inner loops are checked independently; nested
  wrapper loops, seeded history and ambiguous rounds abstain. Error-exit absence requires every
  durable visit of that node to be settled without error; missing controls are never assumed false.
  The dedicated `zeroshot-memory-init` product skill initializes local history through one
  resumable `memory init --directory DIR` command: isolated per-run discovery stays staged until
  separate graph/repository maintenance has covered every card against its recorded current checkout.
  Initialization inventories all local repositories independently of the invoking directory; it has no
  `--cwd` flag. Persisted `--ignore-repos`/`--ignore-graphs` lists match exact inventory aliases and graph
  names/hashes. Per-run repository scopes partition discovery and maintenance; shared unqualified
  graph cards reconcile once, and all selected checkouts are fingerprinted before one atomic publication.
  Source-only historical admissions match repository IDs independently resolved from the current
  checkout/registered remote; historical or folder aliases never prove identity, and remote-bearing
  admissions retain their stronger remote-derived proof.
  The file bridge exports the runtime prompts and reuses its admission validators, never recovers old
  controllers, resolves provider credentials, or writes historical run ledgers. Digest-bound responses,
  source fingerprints, complete coverage, expected revisions and retained tombstones gate publication.
  Session locks serialize advancement; an atomic database receipt recovers interrupted publication
  before checking for new work, and completed sessions return their saved result without replay.
  Historical tool receipts are unavailable; never synthesize exploration evidence from elapsed time.
  The implementation stays internal under `active_memory`; the CLI callback, candidate recall
  capability, and runner lifecycle contracts are exposed only through their owning modules.
  Capsule allocation reuses the resolved
  `ProductionHostingConfig`; instantiated diagnostic storage remains separate from configuration.

- Protocol Rust types are the source of truth. Generated files under
  `protocol/openengine-cluster/v1/` must be regenerated through the Rust testkit, not hand-edited.
- Initial input remains caller-owned and is validated unchanged. A root group may add required
  state fields only when their payload types have deterministic implicit empty values; verification
  proves this and reduction materializes missing null, string, array, and recursively empty record values.
- Map reduction collects a promoted field only when every item wrote it in that item scope. A worker
  error leaves an incomplete collection unchanged while its controls remain available to authored
  guards; never substitute inherited arrays, null placeholders, or shorter partial collections.
- Each full-v1 loop round starts with fresh error, signal, group, and output-channel facts for its
  body at that map scope. Caller state persists across rounds; internal visit markers persist for
  durable replay. Final-round facts replace older facts when the loop returns to its parent.
- Model identifiers are opaque provider-owned strings. Do not infer a harness from a provider/model,
  maintain runtime model catalogs, or validate provider availability. Admission may reject only known
  incompatible harness/provider pairs.
- Full-v1 loops may declare `maxIterationsInput` as an integer path in the initial input.
  Reusable profiles verify the authored default `maxIterations`; native-v2 run admission binds a
  present positive safe integer from caller input and re-verifies the concrete graph. An absent
  value retains the default. The generic execution and loop-entry budgets remain authoritative.
- Runtime selection requires caller-authored `harness`, `provider`, and `model` values.
- The run-level `harness`/`provider` pair is the default lane. An agent node binding may carry a
  typed `lane` override (`RuntimeLane`); `git_delivery` bindings cannot. Candidates build one
  adapter per distinct effective lane and route each node to its own lane's adapter, never falling
  back to another lane; adapters keep their provider fixed and never switch it per turn. Provider
  access is derived per node from its effective lane, and connection requirements are the union
  over nodes. Targets advertise `openengine.node-runtime-lanes/v1`. The CLI refuses to submit or
  store a lane-bearing plan on a target without that exact marker kind, after discovery and before
  acquiring an access token, so no request body is sent. An absent marker, or one of a different
  kind, never fails discovery, but a marker object with extra fields does. The marker stays exactly
  `{"kind": ...}`: later capability data, such as which harnesses a target can run, gets its own new
  extension key, because older clients ignore unknown extension keys but reject unknown fields
  inside a marker. `TargetDiscoveryExtensions` must keep ignoring unknown keys. Local `run`/`resume`
  and `zeroshot acp` check every lane's executable on the captured search path before launch,
  report a missing one as a usage error, and never probe login state. Checkpoint resumes defer
  lookup to provider startup after the controller restores the selected workspace under its lease. Each `zeroshot acp` session
  checks against its own workspace when it opens and returns the error as ACP invalid params; the
  startup check also runs, but only when every search-path entry is absolute. The lookup is
  `find_executable`: it follows the OS search that spawning relies on, including Rust's Windows
  bare-program fallback after the authored PATH misses. UI model fields and saved suggestions use
  each node's effective lane; shared model edits are available only within one lane. Review-loop
  reviewers retain the writer's authored lane and model, but not its credentials or session.
- The `gateway` provider resolves `GATEWAY_BASE_URL` and `GATEWAY_API_KEY` through named
  connections. Codex uses Responses with bearer authentication; Claude uses Messages with
  `x-api-key`. Preserve caller-owned base paths and model identifiers without protocol detection.
- Local Codex `openai` runs inherit the user's configured model provider and transport, including
  OpenAI-compatible proxies. Preserve declared `OPENAI_API_KEY` for custom provider authentication
  while supplying the native `CODEX_API_KEY` alias. Hosted runs and explicit OpenRouter/Bedrock/gateway
  selections retain adapter-owned provider setup.
- The Pi lane forwards its own ambient allowlist into the local child the way the Claude and Codex
  lanes do, merging those values only into names the adapter and the declared connections left
  unset, and merging them before credential validation so a key belonging to another lane is refused
  rather than smuggled into the selected provider. Pi resolves credentials from its agent directory
  before the process environment, so a stored login wins when no credentials are declared.
  A declared credential suppresses all ambient authentication forms for that lane.
- Local runs preserve shell endpoint settings, Claude configuration directories and permission controls;
  declared connection values take precedence. Hosted adapters do not inherit ambient settings.
  Endpoint and Claude control variables in declared connections are passed to the harness; active
  transport selectors cannot contradict an explicitly selected OpenRouter, Bedrock, or gateway lane.
- Provider access defaults are materialized from the authored runtime at the execution-placement
  boundary. Omitted connections remain omitted in author-owned local profiles. Local Codex/OpenAI,
  Claude/Anthropic, Copilot/GitHub, Pi/Anthropic, and Pi/OpenAI lanes reuse native login state
  without inventing a connection requirement; contained placements and non-native lanes add only their canonical
  missing requirements. Explicit
  compatible authored fields always win. Hosts use
  `native_v2_hosting::materialize_contained_provider_access` before remote profile storage or
  credential resolution. The effective runtime drives the existing connection resolver and
  recovery contracts.
  The local CLI captures the invoking shell in its private one-shot bootstrap, consumes and deletes
  that file before controller effects, and keeps the detached controller's OS environment minimal.
  That environment keeps `ZEROSHOT_RESTIC`, resolved to an absolute path, because it selects
  Zeroshot's own checkpoint tool rather than harness context. A controller that fails before
  readiness leaves one bounded, control-safe startup failure report in its run storage, redacted
  before it is written of the bootstrap's credentials and of every captured shell value except
  short or known-public ones; the waiting CLI consumes it and appends it to its startup error.
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
  Verifier nodes use the same permission handling as workers across all harnesses. Pi exposes no
  permission control at all, so the Pi lane appends no bypass argument and runs no configuration
  probe: its disposable capsule is the boundary, and a contained Pi run additionally pins the
  default tool set and refuses project-local resources. The shared prompt
  renderer tells workers to use repository-declared setup, await terminal command status, and keep
  standalone executable tools in the run's `ZEROSHOT_TOOLS/bin` when provided. All workers and
  verifiers share the prepared workspace and can write files. Review instructions prohibit repairs
  and warn that other nodes may execute concurrently; this is an instructional boundary. Missing
  dependencies are reported with environment evidence rather than unrelated code repair requests.
  Hosting embedders may attach an `AgentContextProvider` through `ProductionHostingConfig`.
  The runner samples its nonsecret context for every agent execution, including node-instance
  revisits and auxiliary agents, and all four harnesses prepend it through the shared invocation
  prompt methods. Host context stays outside authored graphs, runtime plans, durable invocations,
  and checkpoints. Default local/direct composition attaches no host context.
  Shared inspection owns bounded JSONL exchange and process cleanup. Each harness owns its native
  policy parser and `apply_permission_default` entry point; `PermissionPolicy` distinguishes unset,
  configured, and unavailable inspection results. Codex browser/computer access controls and explicit
  approval-review features count as authored policy.
- The local target registry initializes `platform` at `https://api.app.zeroshot.sh` with a persistent hosted device identity.
- Local runs select an optional `--cwd` workspace, defaulting to the invoking folder. File arguments
  stay relative to the invoking folder. A Git checkout uses its canonical root; plain, unborn, and
  detached folders are allowed when the graph has no Git delivery. Workspace and active Zeroshot
  state roots must be disjoint, including symlink aliases and missing state suffixes. Recovery
  reuses the stored workspace; local idempotency cannot return a run from a different folder.
  Workspace leases hold shared ancestor locks and an exclusive selected-root lock, so overlapping
  folders exclude each other while sibling folders remain concurrent. Lease guards explicitly
  unlock on drop so fork-before-exec children cannot retain released ownership. Git metadata and
  transport commands discard ambient repository-selection variables before inspecting the selected folder.
- New local PR/merge submissions verify the remote target branch before controller startup,
  after checking for an existing submission. Remote I/O stays outside the shared submission lock;
  admission rechecks idempotency before launch. Preflight reuses the prepared native snapshot and
  delivery binding's resolved credentials; native Git starts in the selected workspace so relative
  search paths resolve consistently. Shared delivery credential helpers preserve declared
  GitHub token precedence and source-only Git authority; forge API tokens remain provider-owned.
- Git checkout and push use credential-free HTTPS or SSH locators independently of a forge.
  The target image includes OpenSSH and a fixed root-owned libnss_wrapper for pinned SSH
  transport. Contained SSH Git uses private per-command user/group records for allocated numeric
  identities; local Git keeps native identity lookup. Contained runs still require an
  exact committed Git source. Explicit source connections
  authorize only transport fields; these values never enter agent environments or durable source
  records. Source-less local candidates construct no delivery adapter.
- `source_code_provider` owns effect-free source and delivery contracts and one provider registry.
  Maintained implementations live in `source_code/adapters/core`; contributed implementations
  live in `source_code/adapters/contributed`. Git is shared transport; forge adapters supply review,
  checks, and merge behavior through the existing delivery nodes and opaque repository/review IDs.
  GitHub remains the maintained forge; its review and merge identity must match the Git remote.
  Capability admission rejects unsupported operations before credential resolution or workspace
  mutation. Pure remote identification uses each registered
  provider's `repository_from_remote` hook; CLI source selection has no separate forge catalog and
  rejects ambiguous matches and unsupported provider versions.
- Conversation-comment delivery uses the same source registry's optional comment authority,
  independently of Git transport and checkout. `builtin.comment-delivery.post@1` has a
  `comment_delivery` binding with no model or lane. Its required inputs are `url` and `body`,
  with optional `reviewScope`. Admission freezes destination/scope from required caller-input
  bindings; the trusted publisher rechecks them before effects. At most one publisher runs,
  outside loops/maps, after every other execution and cleanup, with no later executable work.
  Native output and review results preserve the complete `receipt` record, including absent optional
  provenance. Terminal success must contain that exact native output. Generic local comment graphs need no
  Git source; contained placement requirements remain unchanged.
- Comment publication is write-once per root delivery identity and node. A scoped publication
  store durably records intent before POST and confirmation before a receipt. Local/direct
  files live outside workspace/checkpoint rollback; `HostedWorkspace.comment_publications`
  supplies host-owned persistence. Matching retries reuse the original comment; changed content
  refuses. Lost responses reconcile exact marker, target, author and body hash; unresolved
  publication never authorizes another blind POST. Comment credentials remain private to the
  native adapter. Source-less receipts omit `sourceRevision`.
- `--task-from` prepares a review before graph dispatch, resolving an issue/PR URL into ordinary
  `url`/`context` input and exact source. Workers have no mandatory `task` field. Review context
  always includes `reviewScope`: empty for an issue, complete base/head identity for a PR.
  `issue-review` retains the basic issue-proposal report. `pr-review` requires complete PR scope
  and `linkedIssues` captured from a bounded GitHub GraphQL snapshot, including manual and keyword
  closing links. Incomplete/erroring linked metadata refuses preparation; plain mentions remain
  narrative references. Its classifier returns zero or one `{plan}` task, a `maxItems: 1` map
  runs the reproducer when required, and a final code checker authors a comment with a 150-word
  instruction budget. Reproducer evidence begins with short `Reproduction:`/`Setup:` sentences
  covering the actual workflow/result and startup failures or workarounds, even resolved ones;
  the checker preserves them in the comment. PR headers omit a source revision already named
  by their captured head. Classifier/checker are inspection-only verifiers; the reproducer is a step
  that can prepare the full local stack and browser tests without repairing tracked source.
  Mapped runtime failures and overflow stop before publication; failed checks and environment
  blockers return evidence for the report. All model choices remain caller-authored; the supplied
  PR runtime binds the three independent agents to `gpt-6.1-sol` with `xhigh`. Negative findings
  are successful reviews. Selecting either review template defaults to comment
  delivery. PR review currently requires comment delivery for comparison preparation; issue review
  also supports `--delivery none`. Local review never switches/resets a worktree and rejects tracked changes or a
  mismatched PR head. Explicit submission-key retries retain prepared input/source and authored
  configuration within the selected target/credential identity. Human login attempts rotate a private
  cache generation; OAuth refresh preserves it. Machine credentials and organization selection have
  separate hashed namespaces; snapshot files contain no credentials. Target resolution uses an authenticated read-only route; hosted access uses
  host-owned GitHub authority, while direct requests carry explicit ephemeral source authority.
  PR preparation fetches the exact head/base, retaining the base under a private Git ref so
  checkpoint normalization preserves comparison objects. It uses the admitted publisher's bound
  URL/scope, leaving arbitrary caller input keys unreserved. Local fetch/ref writes run only under
  the controller's workspace lease, after checkpoint restoration and before dispatch.
  Targets advertise comment delivery and
  task resolution through separate versioned discovery extensions.
- Built-in template selection defaults to PR delivery for `software-change`, comment delivery for
  `pr-review` and `issue-review`, and no delivery for the other templates. Explicit supported delivery choices override
  these defaults. Run, profile creation, template export, and editor ordering agree; Python leaves
  unspecified delivery to Rust rather than maintaining a second default catalog. Custom graphs and
  saved profiles retain their authored delivery nodes and bindings.
- Named targets store only endpoint, access mode, and login identity. Named runs resolve repository, branch, exact remote revision, and worktree dirtiness client-side from the selected Git worktree plus per-run overrides; target records never bind repositories.
- `ZEROSHOT_API_KEY` selects hosted machine authentication for the whole target CLI. It stays in
  memory and HTTP Authorization, bypasses OAuth and the credential store, and never falls back to
  human login. `ZEROSHOT_ORGANIZATION` supplies the optional organization selection header.
  Machine keys require the discovered `zeroshot.api-key-authentication/v1` capability. They stay
  out of runtime connections, native snapshots, and child environments. Omitted connection/profile
  scopes follow the authenticated owner; explicit scopes remain explicit. Human `delegation`
  commands use the Admin audience; target login verifies that account before persisting refresh
  custody, including accounts without an organization.
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
- Shared provider process code owns streamed-turn exchange and settlement for Claude and Pi,
  including cancellation, token usage, and confirmed cleanup. Process evidence distinguishes
  incomplete input, failed completion, and stderr-only diagnostics. Incomplete input fails that
  attempt and cannot itself authorize continuation; each native transcript retains its documented
  retry policy, event parsing, and final-answer semantics. Shared provider text publication batches deltas, redacts credentials
  across event/stream boundaries, and emits bounded UTF-8 records. `HarnessProcessEnvironment`
  owns the minimal nonsecret environment for Claude and Pi in local and hosted composition.
  `TokenUsageDelta::checked_add` owns bounded provider-neutral usage arithmetic. Overflow or
  malformed claimed Pi usage makes the turn total incomplete permanently rather than publishing
  an earlier partial total. Native
  authentication merges suppress all ambient forms of a credential group when a connection
  declares one form. Provider
  documents use atomic private-file replacement with descriptor-based ownership; Linux pins the
  private home before publication and never follows a worker-created destination link.
- Structured-output correction runs in the same provider session under the node's existing
  execution policy; local response validation remains authoritative.
- Provider continuation is bounded: Claude continues once after `system/api_retry`; Codex continues
  once after a terminal execution error. Both send literal `Continue` in the same session when one
  exists. Structured output receives at most two correction turns before `malformed`.
- Pi drives one `pi --mode json` process per turn, with the node prompt on stdin so a large input
  payload can never exceed the platform argument limit. Model identifiers stay caller-owned and Pi
  owns their resolution. Its provider lanes are `anthropic`, `openai`, `openrouter`, `bedrock`, and
  `gateway`; the gateway lane synthesizes a private `models.json` that registers its own Pi provider
  against the caller's base URL and references the declared key by interpolation, so no secret enters
  the file and no Zeroshot-owned model catalog exists. It registers its own provider rather than
  retargeting a built-in one because Pi honors `api`, `apiKey`, and `baseUrl` on a registered
  provider while ignoring the first two on a built-in one; that is what lets a declared `GATEWAY_API`
  name the wire protocol (`openai-responses`, `openai-completions`, or `anthropic-messages`) and
  reach any endpoint Pi implements without probing it. The registered model entry carries the
  caller's identifier, the chosen protocol, and a `reasoning` flag mirroring the binding's authored
  effort; context window, output limit, and cost stay Pi-owned defaults. The same file is Pi's only
  caller-owned-endpoint mechanism, which is why it also backs a declared or ambient
  `ANTHROPIC_BASE_URL` on the `anthropic` lane and `OPENAI_BASE_URL`/`OPENAI_API_BASE` on the
  `openai` lane: those overrides retarget the endpoint and keep each built-in provider's own
  protocol, mirroring the `ANTHROPIC_BASE_URL` escape hatch Claude Code already offers and the
  endpoint variables the Codex lane inherits. A
  stored `auth.json` credential outranks a `models.json` `apiKey` on any provider, so the declared
  key must still reach the child through the environment, and a declared endpoint or a declared lane
  credential ends native-login reuse so a stored credential is never sent to a caller-owned host and
  never outranks the declared one. An endpoint override interpolates the lane's key field only when
  the connection supplies it; the anthropic override omits `apiKey` when `ANTHROPIC_API_KEY` is
  absent so Pi's own `ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_OAUTH_TOKEN` environment auth still resolves.
  Pi home overrides resolve native tilde shorthand against the captured user's home before
  detachment and leave `file://` decoding to Pi. A native-local lane reuses
  the user's stored Pi login from its agent directory; every other lane receives a private agent
  directory so no ambient configuration, catalog, or credential is inherited. Session identity is a
  bounded SHA-256 of the node-instance or execution identity, because Pi accepts
  `[A-Za-z0-9._-]` with an alphanumeric first and last character and imposes no other bound. Pi reports a failed or aborted model response in its event
  stream while still exiting successfully, so the terminal event's stop reason decides provider
  success and exit status alone is insufficient. Pi exposes no response-schema flag, so the Pi lane
  carries the machine-readable provider schema in the prompt rather than the `NodeResponseContract`
  object, and correction turns restate that same schema so the retries converge. The schema dialect
  follows the lane's wire protocol: the `anthropic` and `bedrock` lanes and a gateway
  `anthropic-messages` protocol use the neutral schema, while the `openai` and `openrouter` lanes
  and the gateway `openai-responses`/`openai-completions` protocols use OpenAI strict mode; a
  gateway lane must name its protocol through `GATEWAY_API`. Local validation remains authoritative
  and corrections stay bounded to two turns.
- Copilot uses the pinned CLI's headless JSON-RPC protocol 3, with provider `github` and
  caller-owned model IDs. Remote session synchronization is disabled with `--sync-session off`.
  Structured output and corrections share one session; node-instance
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
  After an accepted answer, a valid `session.detach` refusal or peer EOF is logged and keeps the
  outcome, including a closed stdin pipe during that request. Malformed RPC replies/events, other
  RPC writes, cancellation, durable output and cleanup failures still fail closed.
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
  removes the full stage. Small catalogs atomically map logical checkpoint IDs to Restic snapshots.
  Restic drains progress and private stderr completely with bounded memory, retaining only the
  latest summary under a per-record size bound. Errors identify the operation and failure stage
  or exit status without private child output; failed backups never replace a published checkpoint.
  Versioned execution seeds live in separate files and are read only for selected continuation.
  Restart restores the latest published snapshot without importing a seed. Restore staging is a
  private sibling of the workspace so final replacement stays on one filesystem. Successful
  lineages delete their catalogs and repository only after durable terminal success; failed lineages
  retain both for resume. Disposable checkpoint staging cleanup is best effort after confirmed
  runtime cleanup and before observer ledger access. Copied read-only directories are made writable
  only inside private scratch; staging cleanup failure cannot block terminal truth or status.
  Hosted failed runs likewise collect checkpoint staging as best effort after process cleanup;
  retained recovery still requires runtime cleanup, workspace ownership transfer, and durable
  recovery metadata. Force-stop and explicit discard retain authoritative lineage deletion.
  Hosted factories advertise checkpoints only when they implement storage and restore. Snapshot
  restore requires exclusive workspace ownership and preserves the checkout root and Git identity.
- Hosted workspace recovery advertises `resume`, `checkpoints`, and `discard_workspace` in the
  separate `openengine.hosted-workspace-recovery/v1` outer discovery extension, leaving the strict
  `zeroshot.hosted-runs/v1` route object unchanged for older clients. POST bodies and results match
  the OECP `Run*Params`/`Run*Result` types. These authenticated Platform routes remain available after
  capsule disposal. Platform resolves the admitted connection references freshly; hosted recovery does
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
  The supervisor records every node error in durable logs before settlement, including handled
  failures and retries. Summaries preserve error code, typed refusal/malformed reason, elapsed time,
  and returned error context; provider adapters redact credentials before returning details. Runtime failure,
  runtime loss/restart reconciliation, and force-stop use the same completion/log transaction for
  every active node. Failed runs also retain a run-wide error log, including before graph dispatch.
  A fatal supervisor error logs its typed cause, never a panic payload, before runtime cleanup, so
  unconfirmed cleanup or a lost terminal append keeps it.
  An actual node error remains logged if a parallel winner voids that execution; intentional
  cancellation is not presented as a new crash. Unconfirmed cleanup preserves a bounded best-effort
  error log without completing the still-active execution or masking the primary failure.
  Detailed provider, validation, and delivery errors retain their useful explanation in ordinary
  run logs. Adapters redact their known credentials before returning error details; the supervisor
  preserves these through ordinary completion logs. Capsule transport retains details as an existing
  output event before reducing the failure to its wire code; the existing bounded cancellation queue
  retains errors during draining; capsule terminal metadata preserves bounded error records too. Retries, corrections, and typed delivery
  outcomes log detail separately only when completion would lose it. Correction prompts stay out of
  diagnostics. Public error formatting is bounded and control-safe without suppressing causes.
  These public summaries stay separate from private operator diagnostics.
  Existing Error output and safe-log records may carry `FailureDiagnosticKind` metadata; their text
  is the public-safe explanation. The ordinary ledger projection retains a bounded `FailureContext`
  and attaches it to failed terminal results in the same append transaction. Status, watch, SDK,
  shared UI and archived history carry that result directly. Known runtime causes remain distinct
  from recorded node failures; recovered/voided evidence never establishes terminal causality.
  Diagnostic overflow marks context incomplete independently of token usage. Producer and capsule
  records retain their existing execution/cursor identity; no separate diagnostics API or store exists.
  When history is unavailable, the viewer separates current failure status from recorded playback
  and offers log navigation only for loaded evidence.
  Exhausted corrections mark only the final rejected response. Failure-only parser previews redact
  before cleanup and exclude successful answer text. Known unconfirmed cleanup keeps its fatal
  classification while bounded best-effort publication records any missing diagnostic coverage.
- A failed durable-output bridge cancels and drains its provider immediately. Fatal supervisor
  errors and task panics publish their private primary diagnostic before recovery waits or stderr
  writes, then close owned work and attempt runtime cleanup before durable failure. Recovery failures
  publish a separate private diagnostic without replacing the primary cause. If persistence is
  unavailable, the controller retains a minimal `runtime_failed` status at the
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
  all agents share the workspace identity. Delivery confirms its own supplementary-group helper cleanup
  before success, repair, error, cancellation, or panic can release that identity; unconfirmed cleanup
  is fatal. A caught delivery panic settles as a node crash after confirmed cleanup, without exposing
  its payload. Allocation requires an idle writer domain before authenticated checkout and rechecks
  it before deleting the workspace or releasing its identity lease; failed cleanup retains both.
  Allocation retains cleanup authority before checkout starts, including through startup errors and
  cancellation. Controllers confirm destruction before recording an allocation failure as terminal.
  Local Git retains the caller's OS identity, separately from fixed delivery commit authorship.
- Delivery clears the environment of its Git and GitHub CLI processes, so local composition pins
  delivery Git and, when the GitHub adapter is selected, `gh` to the files `find_executable` finds
  on the captured search path. Local `run`/`resume` reject a missing one as a usage error with the
  lane executable check; checkpoint resumes resolve them when the controller composes the restored
  run, before any agent starts. Hosted composition keeps its fixed `/usr/bin` programs.
- Local controllers may use `ZEROSHOT_GITHUB_TOKEN_FILE` for renewable trusted GitHub delivery
  authority. The private file stays outside the checkout and is reopened at delivery entry and after
  authentication rejection. Explicit `GH_TOKEN` wins and invalid explicit values fail closed. The
  host rotates the file atomically; unchanged rejected issuances wait for rotation. The file setting
  is removed from provider context, travels only through the consumed private bootstrap, and is
  freshly selected on resume, never persisted in recovery or run history.
- `ZEROSHOT_DELIVERY_CHECK_COMMAND` selects one private absolute executable outside the checkout
  for trusted local publication, review observation and merge validation. Its capability travels only through the consumed
  private bootstrap, never provider context or recovery metadata; resume selects it freshly. The
  controller owns process cleanup and passes public candidate facts without ambient credentials.
  Authoritative merged/closed observations settle before hooks or late feedback; a completed remote
  merge cannot become a new repair merely because main now contains the fix.
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
- Git diagnostics retain bounded, credential-redacted command/status/stdout/stderr details.
  Capture and native transport share credential-name selection for tokens, passwords and API keys,
  respecting operating-system case rules. Trusted Git commands pin author and committer identity
  after applying native transport context and keep automatic GC foreground on old and current Git.
  Private Git protocol stdout stays separate from printable diagnostics; parsing and failure
  classification must preserve raw evidence before credential redaction. Final delivery feedback
  also redacts the invocation's source credentials and its current refreshed issuance;
  unfamiliar failures use the existing optional `repair_required` signal. Only successful receipts
  require complete remote identity. Live output splits large UTF-8 diagnostics into bounded records.
  Delivery checks unresolved index entries before `git add --all` and completes resolved merges even
  when their tree has no staged difference. Other unfinished Git operations return raw status for
  repair before staging or reconciliation. Delivery does not inspect or filter user-installed tooling.
- GitHub delivery treats aggregate merge policy and required contexts as authority, waits through
  merge queues/deferrals, and succeeds only after observing the exact merged result. Merge methods
  must satisfy repository capabilities, base-ref branch protection, and every active applicable
  ruleset returned by GitHub. Either branch-protection view can require linear history; neither
  overrides a stricter restriction. Direct merge submission reads all rule pages with an independent
  cursor; PR observation and queues do not require direct-method discovery. Incomplete rule reads and
  empty method intersections stop merge submission with policy diagnostics. Merge queues choose their
  own method. Queue admission uses explicit `enqueuePullRequest` with the validated PR node ID and
  exact `expectedHeadOid`; enabling auto-merge alone is not enqueue confirmation. Completed transient
  submissions retry after fresh exact-head policy checks; retries retain earlier uncertainty so an
  empty snapshot cannot authorize repair. Before a merge
  delivery returns repair, refusal, error, or cancellation, trusted authority confirms withdrawal
  of queued/auto-merge effects or the exact already-merged revision. Failed withdrawal never
  authorizes agent repair. Authorities that schedule deferred merges must override
  `withdraw_merge_request`; its default is valid only when no autonomous merge effects exist. Merge commands retain contained process cleanup and use the GitHub API error boundary,
  never Git repair; changed gates cannot mask a permanent rejection. Configured
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
- The built-in `software-change` graph has one `worker` inside its ten-round loop for initial
  implementation, review repairs, and Git/CI repairs. Graph-owned `phase.after` identifies the
  latest feedback source; the empty initial phase and continuation array are implicit state, not
  caller input. Empty maps continue a rejected round without another agent execution. Every repair
  returns through both existing reviewers before delivery. CLI uniform runtimes without an explicit
  session scope and new UI templates use `node_instance` for this worker and `execution` for
  reviewers; exact runtime plans and explicit session scopes remain caller-owned. Saved graphs are
  unchanged. The final rejected round fails without starting an unreviewable extra repair.
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
- The built-in `auto-research` graph uses five model roles per normal round: three independent
  read-only scouts (explorer, synthesizer, challenger), one researcher, and one fresh read-only
  reviewer. Template runtime defaults retain a separate `node_instance` session for each scout and
  the researcher; the reviewer defaults to `execution`. Explicit caller scopes still win. CLI and
  profile-editor templates use the same defaults. Optional `options.iterations` is a positive safe
  integer, default ten, subject to generic execution and loop budgets. No bootstrap, staging,
  finalization, or separate audit model pass runs.
  The researcher owns planning, parent verification/restoration, experiments, ordinary repairs,
  retained state, and compact records at the shared workspace root's `.zeroshot/research`, even
  when the task is in a subdirectory. Scouts check this location and cited task records before
  declaring history absent. Source archives retain actual mutable bytes and path/hash manifests;
  missing optional archives become unavailable rather than blocking a sound current result.
  The researcher returns a substantive `researchReport`; the reviewer returns `reviewFeedback`
  and `accepted`, `revise`, or `stop`. Its native verdict is carried in `review.verdict` beside the
  feedback; accepted warnings never reopen a round. Revisions return to the same researcher without
  re-scouting or consuming another research round, followed by fresh review. The work/review loop
  has a 64-pass safety budget. Native review outcomes alone establish independent acceptance;
  there is no second file-based audit counter. Cosmetic or recoverable historical metadata issues
  are advisory; correctness, evidence, protected boundaries, and safe retained state remain binding.
  After two accepted non-adoptions, or diminishing returns over
  `max(1, ceil(iterationLimit / 10))` accepted rounds on one approach, reconsider a fresh mechanism,
  an archived parent, and a discriminating measurement with actual round IDs. No forced switch,
  universal gain threshold, or task-specific adoption policy is added by the graph.
  The template supports local results or `push@1`. The reviewer authors push metadata; the single
  trusted delivery node publishes each accepted round after all writers settle. Research agents
  never use Git. Delivery errors or repair requests, exhausted review, and unrecovered execution
  errors are explicit failures, not accepted experiments. Saved profiles and admitted runs retain
  their stored graph, runtime and session policy.

- Target images apply current Debian Trixie package updates and install a checksum-verified upstream
  GitHub CLI. Image tests exercise GraphQL pagination with the installed CLI before publication.
  They ship one Rust toolchain baseline plus Node.js, Python and shared native build tools. They expose
  Rust through the fixed runtime PATH without a shared writable Cargo cache; `/opt/rustup` is
  root-owned without group/other write access. Explicit user toolchain settings and installations
  take precedence. Runtime toolchain smoke tests inspect ownership and mode bits, then compile
  native fixtures as an isolated user with a read-only root and fresh home.
- Native-v2 admits concurrent writers in parallel branches and map items. Writers share the run's
  workspace owner identity; hosted session cleanup tracks an immutable supplementary group marker
  per session. Authored graphs coordinate overlapping edits. Admission rejects Git delivery that
  can overlap another agent or delivery, including verifiers. Every executable node participates
  in workspace checkpoint and final delivery accounting.
  A delivery receipt certifies success only if every other writer settled before delivery started.
  Unconfirmed process cleanup is a fatal runtime failure, including after cancellation; it cannot
  be reduced to a retryable node crash or an authored parallel-join void.
  Retained workspace handoff commits when the source workspace moves to its successor. A later
  allocation failure preserves that successor for recovery; unconfirmed cleanup keeps its durable
  run nonterminal until replacement-controller reconciliation confirms cleanup.
- Workers and verifiers use one canonical workspace, including ignored dependencies, build outputs
  and services. Provider session homes and scratch directories remain separate; individual process
  cleanup uses session group markers so it cannot terminate a peer or a startup service. Workspace
  roots are run-owner `0700`, runtime roots supervisor-owned `0711`, and production ledgers `0600`.
  Session homes disappear only after confirmed cleanup; node-instance homes survive loop revisits.
  On Linux, provider `TMPDIR` is a supervisor-owned `/tmp/zs-<16 random hex>` symlink to the
  execution scratch, so project Unix sockets fit `sun_path`. Files stay in the execution root and
  the link is removed with it; an untrusted `/tmp` or any link failure keeps the full path.
- An optional top-level run `environment` carries public setup/startup scripts, nonsecret variables,
  and explicit hook connection references. Profiles and runtime plans have no environment field.
  The CLI accepts `--environment FILE` or `--no-environment`; hosts may resolve an omitted definition
  before acceptance, while an explicit empty object selects the base environment. Core stores no
  saved environment catalog and has no resource discovery or local environment UI. Hosted
  preparation is accepted asynchronously and owns its
  controller lease before allocation. Setup runs as root before checkout or checkpoint restore;
  startup runs as the workspace owner in the restored checkout before graph dispatch. Both rerun
  on each resume attempt. Hooks have fifteen-minute limits within one thirty-minute preparation
  budget, emit bounded redacted run logs, and fail before agent retries. Force-stop signals
  preparation before waiting for cleanup; failed cleanup retains authority for replay/recovery.
  `ZEROSHOT_TOOLS/bin` precedes the base PATH for hooks and all agents; shell exports do not persist.
  Startup services retain a separate run-owned process marker and stop at run cleanup. Checkpoints
  retain files and graph state, not services, Docker state, or conversations; users reconstruct
  services idempotently and keep live database state outside file checkpoint guarantees.
  Local runs reject hooks and use the invoking environment. Direct Docker setup modifies its
  operator-owned target container, shared by that target's runs; separate target containers are
  needed for independent OS dependency sets. Platform supplies its own disposable execution placement.
  Image-owned harness wrappers pin their Node interpreter independently of the project PATH.

- Native local CLI and in-process execution support Unix and Windows. Shared OS facilities live in
  `execution::platform`; local controller transport selects Unix sockets or private Windows named
  pipes behind one NDJSON protocol. Unix sockets retain their in-storage path when it fits the
  platform byte limit; endpoint derivation normalizes redundant separators and `.` components.
  Longer paths use `/tmp/zeroshot-<euid>-<sha256(normalized-storage)>/socket` independently of `TMPDIR`.
  Each run gets its own endpoint directory, so reserving a UID-only directory cannot block all runs.
  Endpoint directories are created as `0700` and validated for current-user ownership and no
  group/other access through the existing private-file check. Sockets use `0600`; listener cleanup
  checks device/inode identity, removes empty temporary endpoint directories, and runs before
  releasing controller ownership. Durable state
  stays in the configured state directory. Windows state uses protected current-user/SYSTEM ACLs, rejects
  reparse points, and pins volume/file identity. Provider and delivery descendants belong to
  kill-on-close Job Objects before their first instruction. Detached controllers run on a new
  windowless console shared by their console descendants, ignore Ctrl-C/Ctrl-Break delivered to it,
  inherit no caller handles, and resume only after proving they escaped every caller Job;
  restrictive Job policies reject controller startup. Windows config defaults to
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
- A foreground run whose watch closes without a terminal snapshot settles from run status, which
  Ctrl-C detaches like observation. A failed foreground run's process error carries its latest
  bounded, control-safe error-log records and the `zeroshot logs` command. That read is best
  effort, deadline- and Ctrl-C-bounded, and never replaces the failure.
- Graph verification errors display their first safe diagnostic through the shared verifier error,
  so local validation and hosted rejection report the same cause.
- Do not hand-edit `docs/zeroshot-cli.md` or `docs/zeroshot-cli.html`; regenerate with
  `cargo run -p zeroshot --example generate_cli_docs -- --write` and verify with `--check`.
- Page descriptions live in Markdown front matter; generated reference descriptions live in
  `scripts/docs_hook.py`. `docs/overrides/main.html` renders social metadata and breadcrumbs.
  Fonts are self-hosted in `docs/assets/fonts/` with their licenses.
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
  That migration, and a second pass after `mike deploy`, point each minor page's canonical link at
  the same Current page on `https://zeroshot.sh/docs/`, or at its own minor URL when Current lacks
  the page, and rewrite older GitHub Pages origins in minor sitemaps. `SITE_URL` in
  `scripts/docs_versions.py` must match `mkdocs.yml`.
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
  from public run status and logs. `ProductionHostingConfig::operator_diagnostic_output` accepts an
  optional `OperatorDiagnosticOutput` created before target construction. Its nonblocking channel
  exports the same normalized records from checkout, Git push, and fatal runtime producers; the
  two-record private snapshot remains independent. The channel retains at most 128 records and
  reports overwritten records through Tokio's `Lagged` error. Hosts own continuous collection,
  durable storage/export, deployment identity, timestamps, retention, and shutdown draining; this
  in-memory handoff cannot guarantee preservation across abrupt process termination. Treat lag as
  incomplete diagnostics. Namespace IDs by target process/attempt and keep collected data private.
  `OperatorDiagnosticJsonLines` optionally drains those records on a dedicated writer thread,
  flushing each line and reporting overflow. `target serve --operator-diagnostics-json` enables
  stdout output. Writer shutdown is bounded; it cannot guarantee delivery after abrupt termination.
  Output has no deployment metadata or cloud-specific envelope.
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
| Hosted/platform composition   | `zeroshot/src/native_v2_platform.rs`, `zeroshot/src/native_v2_hosting.rs`                                                                                                                         |
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
  Hosted workspaces use the `platform` kind with their existing opaque workspace IDs. The tag
  rename starts a new browser storage namespace; earlier drafts and preferences are not imported.
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
  and the reserved `runtime_failed`/`runtime_lost` failure metadata; UI and Platform readers must reuse
  them after wire decoding. Definitions contain bounded admission and terminal facts, never the
  accumulated execution snapshot; readers accept that legacy field only to strip it. Observer clones
  share its bounded projection cache.
  `profile_ui/runs.rs` supplies local filesystem/list/SSE adapters; observation never creates, repairs
  or recovers a ledger or controller. Preserve ordered execution cursors, explicit gaps and string
  u64 IDs. SSE resumes after `Last-Event-ID` and drains through the terminal cursor before closing.
- Private targets export capability-authenticated `POST /native-v2/history/definition` and
  `POST /native-v2/history/page` over their already-owned observer. Direct and hosted access modes
  cannot use these private exports; Platform applies per-run authorization before using its capability.
  Request bodies carry run identity and cursor, keeping the target query surface unchanged.
- Status reads query only an already-owned controller. Unpersisted runtime failure stays separate
  from durable snapshots/events and marks history incomplete. Missing authority drains retained
  history before an explicit error. Never invent completion; a durable terminal record takes precedence.
  History observation availability is separate from terminal run status: an available runtime failure
  can be emitted at the same cursor while following continues toward the final durable record.
  Platform may remain `collecting` through target/archive handoff; `finished` alone cannot end follow.
- `run-history.ts` projects only the selected history prefix. Rust's canonical reducer supplies
  structural visits/decisions with source cursors and stable identities; JavaScript never evaluates
  guards or invents worker executions. Keep loop visits, retries, and Map items distinct. Projection
  failure leaves recorded worker history readable; verifier rejection is a decision, not a crash.
- Profile and run inspectors share `InspectorPanel`, pointer/keyboard resizing, and one client-only
  width preference. Viewport clamping preserves the preferred width when the available space shrinks.
- Run navigation retains tab-scoped view preferences (node/inspector, execution, cursor, graph
  expansion and viewport) under the workspace identity and run/source identity. History and
  definitions are always fetched anew; unmount cancels polling, reads, streams and replay timers.
  Standalone tab navigation remembers its last run route only for the mounted workspace shell;
  explicit run hashes take precedence. Standalone snapshots use a fresh shell namespace on reload
  so local/target history cannot inherit prior-page preferences. Hosted shells own restoring their
  selected run identity. A saved cursor beyond the bounded batch remains pending through explicit
  Load more; intermediate pages cannot replace it.
- Read-only history reuses `WorkflowCanvas`; groups start collapsed and scrubbing preserves expansion.
  Seek pauses following; transcript scrolling stays live and follows only at the bottom. Keep the
  execution pinned while reading. Node timelines use the shared cursor; transcripts remain bounded.
  Initial/live batches stop at 5,000 events or 8 MiB until explicit continuation. Simulated histories
  remain separate from real runs and profile storage; scenarios/evidence live under `ui/qa/`.
- Visual tokens/fonts follow `zero-cloud/frontend/VISUAL_DESIGN.md`; bundle all runtime assets.
  `embed.html` mounts the same editor/viewer without standalone menus. Its version-1 bridge
  validates the parent window and exact origin, binds workspace authority, and correlates document
  generations and save snapshots. Host service URLs remain same-origin; optional CSRF configuration
  carries cookie/header names only. Platform owns menus, authentication, profile CAS and live/archive
  adapters ([zero-cloud #301](https://github.com/the-open-engine/zero-cloud/issues/301)).
- Shared fetch-based SSE preserves HTTP and stream problem codes, accepted execution cursors,
  bounded frame buffering and owned reader cancellation. A host source change must preserve the
  viewer's history prefix and playback position; it never changes native graph projection.

## Development conventions

- Fix root causes and keep changes scoped.
- Use existing patterns; do not add parallel registries, provider lists, runtime model catalogs, or release
  authorities.
- Keep optional developer and agent analysis tools external to the repository. The npm-delivered
  Zeroshot product skills are the sole product-owned exception; do not add package dependencies,
  hooks, CI gates, other skills, or checked-in state for personal analysis tooling.
- New Rust APIs must respect the four-parameter Clippy ceiling; use request structs rather than
  raising or bypassing the limit.
- Unix tests that create executable fixtures use
  `openengine_cluster_testkit::fixture::write_executable`; its lock handoff covers writable
  descriptors inherited by concurrent process spawns.
- `.opcore.json` owns the full-source Opcore policy. `.github/workflows/opcore.yml` runs the Fast
  and native Rust providers on every PR and main commit. Its two exact PowerShell exclusions reflect
  unsupported parsing; the Windows native CI lane still runs those scripts.
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
  aggregate `required`. On PRs, that aggregate waits for Greptile's latest review check on the exact
  head to complete; findings remain review input for the worker rather than automatic CI failures.
  Merge groups use the completed PR gate because Greptile does not review merge-group commits.
  Native changes also require macOS product and test compilation with the UI
  feature, focused Unix controller socket regression tests, and APFS workspace clone tests that
  `ZEROSHOT_REQUIRE_APFS_CLONE` keeps from skipping. `.github/ci-path-classifier.js` owns
  fail-closed path routing and cross-lane
  producer/consumer dependencies. The native lane runs on Linux and Windows, including real local
  CLI subprocess tests. Windows uses
  `scripts/test-windows.ps1` to run test executables outside Cargo's restrictive Job; the CI-only
  `.github/scripts/test-windows-host.ps1` also starts outside the hosted runner's Job. Linux also executes
  hosted process and filesystem boundary tests as root against its built test binary.
- `.github/workflows/coverage.yml` measures the default workspace, UI-feature Rust tests, and hosted
  root boundary tests on native changes, enforces coverage floors, and publishes LCOV to Coveralls.
  It stays outside the required CI aggregate. Keep Rust test implementations in `tests/`, `tests.rs`,
  `*_tests.rs`, or `*-tests.rs`; do not embed test bodies in production-named source files, because
  production coverage totals explicitly exclude those test-source paths.
- `.github/workflows/release.yml` is the only canonical product release workflow.
- It generates the GitHub Release body from the exact first-parent commits since the preceding
  canonical tag. Every released commit must retain a Conventional Commit squash title ending in
  `(#PR)` and a nonempty release summary. Generation uses the exact release source, and publication
  persists the Markdown as an immutable release asset for byte-exact recovery.
- It publishes native archives/checksums, `ghcr.io/the-open-engine/zeroshot-target`, and
  `@the-open-engine-company/zeroshot`, then invokes Python revision `1`.
- Python revision `1` always produces its GitHub wheel release. PyPI publication is fail-closed by
  default and may be explicitly deferred with `publish_pypi: false`.
- The canonical release dispatches `.github/workflows/release-python.yml` on `main` as a standalone
  run, binds the exact release commit/version and revision `1`, and waits for that returned run ID
  before documentation. PyPI trusted publishing must remain in this standalone workflow, never in
  a reusable workflow. The same entry point handles later SDK-only revisions and exact recovery.
- `.github/workflows/docs.yml` publishes `main` as Current and canonical releases into `vX.Y/`
  after Python revision `1`; later SDK-only releases do not rebuild product documentation.
  The root always selects Current. Legacy `dev` and `stable` redirects stay outside the selector.
- There is no automatic semantic release, release-promotion branch, `dev -> main` flow, or second
  runtime release train.
