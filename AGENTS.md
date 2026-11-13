UPDATE THIS FILE when making architectural changes, adding patterns, or changing conventions.

# Zeroshot v8

Operational guidance for automated agents working on this repository. Install the canonical command
with `npm i -g @the-open-engine-company/zeroshot` or build `zeroshot` with Cargo.

## Critical rules

- Never run `zeroshot run` unless the user explicitly asks to start a run.
- Never use git commands inside validator prompts; validators inspect files and observable outputs.
- Agents are non-interactive. Make autonomous, scoped decisions rather than asking runtime questions.
- Never edit `CLAUDE.md` unless the user explicitly requests it.
- Keep private service implementation, deployment details and acceptance records out of public docs.
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
- That package owns the canonical `zeroshot`, `zeroshot-memory-init`, `zeroshot-graph-design`, and
  `zeroshot-plan` skills and installs managed copies for Codex, GitHub Copilot, Pi, and Claude Code
  at their user scopes. Do not fork skills by host.
  `ZEROSHOT_SKIP_SKILLS=1` opts out of all user-scope skill handling in npm postinstall and
  `zeroshot update`; only the exact value `1` enables it. Executables still install/update, existing
  skills stay untouched, and updates skip skill downloads. Keep default managed-copy protections
  when the opt-out is unset.
- For runtime/workflow, shared skill, or remote plugin changes, check MCP impact using
  [plugins/AGENTS.md](plugins/AGENTS.md).
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
  staged executable, and replaces the running executable in place without privilege escalation.
  Staging and replacement hold an exclusive `.zeroshot-update.lock` beside the executable. Under it,
  an update removes what interrupted updates left there, restoring a moved-aside Restic sidecar when
  none is installed, and reports `updated: false` when the executable and sidecar already hold the
  release's bytes. It installs or refreshes only unchanged managed skill copies and preserves
  conflicts.
  A newer release probes installation-directory writability before downloading any asset; a current
  installation skips that probe. Unwritable installs and archive rejections end with the shared
  version-pinned npm reinstall command; release-lookup HTTP 403/429 uses `@latest` because no version
  was resolved. In-place updates preserve npm's recorded package version. The exact two-executable
  archive layout is a compatibility contract: adding entries strands released updaters.
  Development-placeholder builds refuse self-update. Update downloads use an idle timeout, never a
  whole-request limit, and keep their size caps. Failures carry the complete cause chain and GitHub's
  bounded, control-safe `message`, never request URLs.
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
  reads; final context trimming shrinks the largest evidence collections and retains per-execution transcript tails. Discovery has no findings count
  quota: impact, recurrence, current evidence and usefulness determine admission. Shared transactions
  are bounded by serialized bytes and store capacity rather than a mutation count quota. Their work runs
  before runner closure/cleanup, preserves the durably latched primary outcome and its failure
  diagnostics on ordinary failure or controller restart after confirmed cleanup, and
  stays outside reducer/checkpoint history. All usage remains part of the run. Never turn incomplete
  supplied-memory evidence into an empty manifest. Graph guidance supplements only private provider
  invocations after plan validation; it never mutates authored instructions or graph hashes. Hooks
  retrieve complete cards from a frozen resident index with relevance ranking. Separate per-injection
  ceilings default to 16000 approximate tokens for node graph guidance and 8000 for repository hooks;
  each accepts 1..=64000 and converts one token to four UTF-8 bytes, including framing. There is no
  cumulative conversation/run budget. ID/revision deduplication persists for each provider conversation,
  including compaction; new conversations receive fresh delivery sets. Windows memory storage and initialization use shared
  private-state helpers, including validation of SQLite sidecars. SQLite-created Windows children
  and externally written initialization responses may inherit the token's default owner only with
  an already private ACL and verified private parent; normalize them to explicit current-user
  ownership before use. Initialization creates temporary files with private security at
  creation rather than repairing default-owned files afterward.
  Shared store mutations use expected revisions and retained deletion tombstones.
  Postmortems refresh scoped lessons and pass before/current changes separately from original worker
  exposure. Revision conflicts reinvoke maintenance with retained discoveries under the original
  deadline, defaulting to ten minutes for collection and all roles/retries. Timeout retains only complete,
  host-validated specialist units; maintenance replacements and retirements remain atomic across all
  that specialist's batches. Explicit cancellation fences new publication and drains started writes.
  Maintenance receives the complete original authored graph and nonsecret runtime, inline or through
  complete private definition files; context trimming never substitutes an incomplete prefix.
  When clean source is available, the host stamps repository-bound discoveries/adaptations with
  `source:<actual-clean-HEAD>` evidence. New findings without a verified basis remain unknown; destructive
  maintenance requires local ancestry from the prior inspected source and unchanged clean source;
  unknown legacy, dirty/non-Git, unavailable and divergent/squashed history defer rather than invent
  authority. No fetch, authority ref or extra checkout is involved. Host-owned ActiveMemoryBackend
  adapters supply authenticated snapshot/publication storage.
  Storage reads and queued writes observe cancellation, while started writes drain with bounded storage
  timeouts. Unchanged conflict snapshots stop reconciliation rather than spending model calls on hidden
  tombstones; deferred unpublished findings remain unpublished.
  Codex trusts only the exact normalized hash of the product-owned session hook; never bypass trust
  for unrelated native hooks or write user trust configuration. Callback receipts acknowledge returned
  context, not model compliance. Missing carrier acknowledgement, failed provider startup or bounded
  transport failure makes maintenance evidence unknown. Callback frames have absolute I/O deadlines
  so a stalled child cannot keep the resident broker or run cleanup alive indefinitely.
  Interrupted auxiliary work marks run usage incomplete. Discovery includes authentic main-run
  agent completions and bounded native tool receipts across all graph shapes, retries and locally
  observed resumed work. No failure-count, tool-count or duration threshold gates proposals.
  Specialists assess significance, novelty, causal attribution and reuse; routine corrections and
  generic advice alone are insufficient. Proposals must cite observed evidence for their executed
  agent node; repository lessons also require verified current source. Execution occurrences and
  attempts preserve map-item and retry identity without inferring causal blame from node names.
  Imported history and auxiliary output never become fabricated current-run evidence.
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
- Status/list/watch expose the durably latched primary graph outcome as optional `graphResult`
  while running or stopping, including postmortem/finalization. Finished status carries only
  `terminalResult`. Graph-result logging and its watch event publish with `PrimaryGraphCompleted`;
  terminalization never repeats that graph-result log. Foreground follow ends at Finished or Suspended.
- Initial input remains caller-owned and is validated unchanged. A root group may add required
  state fields only when their payload types have deterministic implicit empty values; verification
  proves this and reduction materializes missing null, string, array, and recursively empty record values.
- Map reduction collects a promoted field only when every item wrote it in that item scope. A worker
  error leaves an incomplete collection unchanged while its controls remain available to authored
  guards; never substitute inherited arrays, null placeholders, or shorter partial collections.
- Each full-v1 loop round starts with fresh error, signal, group, and output-channel facts for its
  body at that map scope. Caller state persists across rounds; internal visit markers persist for
  durable replay. Final-round facts replace older facts when the loop returns to its parent.
- Harness integration lives in `harnesses/`: one registry supplies pure descriptors, provider
  requirements, executable lookup and factories for every adapter. Maintained implementations live
  in `harnesses/adapters/core`; contributions live in `harnesses/adapters/contributed` and register
  explicitly. Protocol harness/provider IDs are bounded opaque strings; registry admission owns
  pair compatibility. Each adapter owns its native environment allowlist and home/path expansion;
  shared process helpers keep only common environment and transport mechanics. Every adapter applies
  `provider_process::apply_execution_environment` after environment merges to expose the public
  `ZEROSHOT_RUN_ID`, `ZEROSHOT_NODE_NAME`, `ZEROSHOT_NODE_KIND` (`step`/`verifier`),
  `ZEROSHOT_NODE_INSTANCE_ID`, `ZEROSHOT_EXECUTION_REF`, `ZEROSHOT_NODE_ATTEMPT` and
  `ZEROSHOT_NODE_ITERATION` labels. Reducer leaf traversal supplies the innermost loop iteration
  (zero outside loops) and reducer attempt through transient `NodeDispatchMetadata`; durable
  invocations/checkpoints and credential connections remain unchanged. Corrections and continuations
  retain dispatch labels; auxiliary agents supply their own identities with initial attempt one and
  iteration zero. Host labels override stale values, including Windows case aliases, without changing
  controller environment or native descendant filtering. Labels are not authorization evidence.
  Existing public core module paths remain available for embedders. Core and contributed adapters
  implement the same complete execution and Active Memory contracts. The distinction controls
  distribution and maintenance ownership only.
  Native memory hook/extension installation belongs to the adapter; shared memory owns the broker,
  frozen recall budgets, receipts and maintenance. Contributions never acquire a separate runtime
  or reduced feature path. Official local builds enable `contributed-harnesses`; target images and
  Platform dependencies disable default features, and target admission independently uses the
  core-only registry even when compiled into a local binary. Targets advertise enabled pairs via
  the separate `openengine.runtime-harnesses/v1` discovery extension; older targets without it
  retain legacy core submissions, while contributed lanes require positive advertisement.
  Local adapters preserve native configuration discovery when provider credentials are declared.
  OpenCode retains its original HOME/XDG paths and native managed, global/project, plugin and MCP
  resources. A private dependency-free native plugin applies declared authentication or endpoints
  to the selected provider after native resolution and checks outgoing authentication headers
  after native header hooks and before the SDK transport. Arbitrary native custom transport code
  remains trusted executable code and may replace HTTP behavior.
  Mask only the selected stored login in private process state; preserve unrelated native auth for
  well-known discovery. The native `auth.set` wrapper supplies its original file or inline authority only within
  that write's async scope, so concurrent model requests retain selected authentication. OpenCode owns
  auth-file writes. The GitHub lane seeds private native OAuth state and retains its native provider
  loader, endpoint metadata and request authentication; declared tokens never replace saved logins.
  Local OpenCode keeps native permissions and adds no `--auto`: native configuration inspection
  starts background dependency work, so use the shared unavailable-inspection fallback without a
  probe, custom policy parser or cache-lock management. Contained OpenCode adds `--auto` in its
  disposable capsule. Active Memory installs a separate native plugin alongside user plugins.
  Explicit `OPENCODE_PURE=true` conflicts with declared authentication because it disables the
  required provider overlay. Per-node `--pure` overrides are reserved because they disable
  runtime-owned authentication and Active Memory plugins.
  Hermes uses its installed canonical executable's interpreter and native `AIAgent`/configuration
  factories in a private process, without starting or attaching its shared gateway. A scoped import
  hook preserves native startup; `--version --help` fails closed if that hook is unavailable.
  Credentials cross private stdin after a versioned handshake. Local profile selection, managed
  settings, plugins, MCP, instructions and permissions remain native. The owned agent, auxiliary
  and delegation factories keep declared credentials authoritative through refresh and retries;
  explicit foreign provider routes refuse before authentication. Native login retains native refresh.
  GitHub binds the native source-token resolver while preserving native exchange, account endpoints,
  refresh and retries; declared credentials use a private native token cache. Bedrock binds private
  native AWS sessions for Converse, Anthropic and Mantle clients, preserving native signing and
  credential refresh. Declared AWS authentication suppresses ambient alternatives and pins declared
  region; native profiles and plugin discovery keep their original paths.
  Structured corrections share one runtime; node revisits follow native compression tips in a
  private native session database. Bind the original native profile through execution and cleanup
  so private database placement never changes SOUL or skills lookup. Captured shell values remain private until native configuration
  requests their names. Declared connections override native dotenv values; provider credentials
  remain private and are available to explicit native configuration lookups.
  Cancellation requests native interruption and confirms terminal jobs, Python kernels, owned
  background work, LSP and MCP cleanup before the shared process-kill fallback. A native cleanup acknowledgement is required
  in addition to process containment evidence. Remote kernel retirement retains known ownership
  until checked native transport confirms settlement; inaccessible evidence stays unknown.
  Caller-selected remote backends retain native containment guarantees. Native token deltas remain visible; unreported
  auxiliary or child work marks totals incomplete. Active Memory attaches native model-facing
  notices after tool-result bookkeeping, preserving native transforms and durable messages.
- Agent bindings may carry a secret-free `harnessConfig` object (64 KiB encoded), with native
  `args` and/or `options` carriers owned by each adapter. The effective node lane interprets it;
  lane changes retain it without translation. Registration supplies pure reserved-control validation,
  and the invocation/session builder applies fresh node-local overrides through native mechanisms.
  Native option names/values and unknown JSON keys remain harness-owned. Connections own credentials;
  model/provider, identity, transport, response contract, required memory hooks and containment stay
  runtime-owned. Never implement overrides by writing user/project configuration. Local Claude/Codex
  native overrides suppress automatic permission bypass and never enter arbitrary flag probes.
  Binding clones, generated reviewers and built-in postmortem specialists retain native configuration
  to avoid broadening restrictions; reviewer credentials and sessions retain their existing ownership.
  Native initialization rejection is terminal in the adapter and keeps redacted diagnostics and node identity.
  Reserve native aliases alongside long runtime-owned controls. Short-option matching belongs to
  the adapter's parser syntax: Pi recognizes exact short tokens, so native values such as `-read`
  must not be interpreted as a joined `-r` option. Contained Pi owns tool selection and project trust
  under both long names and their native aliases; session forks are reserved in every placement.
  Codex exec subcommand tokens stay runtime-owned wherever they occur; literal native option values
  matching those tokens use `--option=value`. Claude session selectors and system-prompt file forms
  are reserved alongside the inline/session controls they replace.
  CLI adapters own native operand boundaries and reject unbound positional prompts/files anywhere
  in `args`. Unknown option names remain native-owned; bind their operands explicitly with supported
  joined syntax (`--option=value`) rather than guessing whether an unknown switch consumes a value.
  Codex image prompt attachments and Pi model-listing invocations remain runtime-owned; they must
  not replace or augment an admitted task through native configuration.
- Model identifiers are opaque provider-owned strings. Do not infer a harness from a provider/model,
  maintain runtime model catalogs, or validate provider availability. Admission may reject only known
  incompatible harness/provider pairs. A caller may declare optional bounded `contextWindow` and
  `maxTokens` on an agent binding; they are declarations rather than catalog knowledge, are written
  only to the Pi gateway model entry when supplied, and stay absent so the provider keeps its own
  defaults otherwise. Never infer either value from the model identifier.
- A `script` binding executes an argv program on an existing step without instructions. The local
  adapter in `native_v2_candidate::script` uses the shared process runner, bounded JSON stdin/stdout,
  authoritative output validation, connection redaction and confirmed process-tree cleanup. It does
  not invoke a shell or inherit ambient provider keys. Local-only placement is enforced both before
  remote submission and at candidate construction. Keep service lifecycle outside the runtime.
- A `decision` runtime binding runs one TypeSafe-compatible System One Choice request on an
  existing verifier, without a harness, tools, host context, or Active Memory. The verifier's
  instructions supply its question and its single signal supplies the options, excluding the
  authored `uncertainLabel`. The complete node input is the state. The native adapter lives under
  `native_v2_candidate::decision`; admission validates its derived choice/probabilities/model
  output and message diagnostic. Exactly `DECISION_ENDPOINT` and `DECISION_API_KEY` come from
  declared connections; neither enters agent environments or durable invocations. Endpoint paths
  and model identifiers remain caller-owned. A tie or winning probability below `minProbability`
  emits the uncertainty label while preserving the original answer. Invalid/incomplete answers
  fail without synthesizing probabilities or signals. Each execution makes one bounded,
  cancellable request with no redirects or hidden retries and records known or incomplete usage.
  Authentication refusal, malformed responses, and timeouts retain their shared worker error outcomes.
  Targets advertise `openengine.decision-models/v1` through a separate discovery extension;
  clients require that exact marker before acquiring access tokens or sending decision plan bodies.
- Full-v1 loops may declare `maxIterationsInput` as an integer path in the initial input.
  Reusable profiles verify the authored default `maxIterations`; native-v2 run admission binds a
  present positive safe integer from caller input and re-verifies the concrete graph. An absent
  value retains the default. The generic execution and loop-entry budgets remain authoritative.
- Run size identifiers are bounded opaque strings. Targets advertise their catalog through
  `extensions.run_size_catalog`, kind `openengine.run-sizes/v2`, with unique `{id, description}`
  entries and an offered default. `target sizes NAME` renders that catalog; `--json` retains its
  structure. Hosted listing reads authenticated discovery for the current organization. Clients
  forward requested IDs unchanged and never use discovery as an admission allowlist. An omitted
  or null requested size remains omitted until the target resolves its default at admission.
  Accepted definitions and history retain a concrete size. Public direct and hosted targets require
  an owned catalog; local/direct serve offers `host`, while private capsules preserve their externally admitted size.
  Request identity stays on the original request so mutable catalog changes cannot invalidate an
  exact replay. Machine resources and lane costs remain target-owned, outside runtime plans.
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
  report a missing one as a usage error, and never probe login state. Checkpoint resumes check
  instead when the controller composes the run after restoring the selected workspace under its
  lease, before admission; the CLI's controller startup error carries the same lane message and
  no successor remains. Each `zeroshot acp` session
  checks against its own workspace when it opens and returns the error as ACP invalid params; the
  startup check also runs, but only when every search-path entry is absolute. The lookup is
  `find_executable`: it follows the OS search that spawning relies on, including Rust's Windows
  bare-program fallback after the authored PATH misses. UI model fields and saved suggestions use
  each node's effective lane; shared model edits are available only within one lane. Review-loop
  reviewers retain the writer's authored lane and model, but not its credentials or session.
- The `gateway` provider resolves `GATEWAY_BASE_URL` and `GATEWAY_API_KEY` through named
  connections. Pi, Copilot, OpenCode and Hermes additionally require an authored `GATEWAY_API`
  selecting their native Responses, Completions or Messages transport; they never infer it from a model.
  Codex uses Responses with bearer authentication; Claude uses Messages with
  `x-api-key`. Preserve caller-owned base paths and model identifiers without protocol detection.
- The Codex and Claude `foundry` lanes share `AZURE_FOUNDRY_API_KEY` with exactly one of
  `AZURE_FOUNDRY_RESOURCE` or `AZURE_FOUNDRY_BASE_URL` (a resource root). Claude translates them to
  its native `ANTHROPIC_FOUNDRY_*` settings with `CLAUDE_CODE_USE_FOUNDRY=1` and drops the shared
  names; Codex configures an Azure v1 Responses provider that sends the key in `api-key`.
- Local Codex `openai` runs inherit the user's configured model provider and transport, including
  OpenAI-compatible proxies. Preserve declared `OPENAI_API_KEY` for custom provider authentication
  while supplying the native `CODEX_API_KEY` alias. Hosted runs and explicit OpenRouter/Bedrock/Foundry/gateway
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
  transport selectors cannot contradict an explicitly selected OpenRouter, Bedrock, Foundry, or gateway lane.
- Provider access defaults are materialized from the authored runtime at the execution-placement
  boundary. Omitted connections remain omitted in author-owned local profiles. Local Codex/OpenAI,
  Claude/Anthropic, Copilot/GitHub and its OpenAI/Anthropic/OpenRouter BYOK lanes, Pi/Anthropic,
  Pi/OpenAI, Pi/GitHub, and native-local contributed lanes reuse native login state
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
  short or known-public ones, including certificate trust paths so unreadable files stay named;
  the waiting CLI consumes it and appends it to its startup error.
  Preparation before readiness, including a checkpoint restore, has one finite deadline: 600 s, or
  `ZEROSHOT_PREPARATION_TIMEOUT` whole seconds from 1 to 86400. The controller stops a restore
  that is still extracting there; workspace replacement, once begun, runs to completion so it is
  never stopped halfway on purpose. The CLI waits a short stop grace longer, then stops the
  controller's process group (Unix) or process (Windows) and names the deadline. Handoff waits keep
  their separate short bound.
  A later local submission or resume scavenges orphaned bootstrap files after the bounded controller handoff
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
  separate sixty-second/4 MiB budget, never send a
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
  revisits and auxiliary agents, and all registered harnesses prepend it through the shared invocation
  prompt methods. Host context stays outside authored graphs, runtime plans, durable invocations,
  and checkpoints. Default local/direct composition attaches no host context.
  Shared inspection owns bounded JSONL exchange and process cleanup. Each harness owns native
  permission handling and any parser it needs; `PermissionPolicy` distinguishes unset,
  configured, and unavailable inspection results. Codex browser/computer access controls and explicit
  approval-review features count as authored policy. Unavailable inspection carries a cause; the
  harness that withholds its bypass reports it once through `report_unavailable_inspection` as a
  redacted, bounded error-stream record without a failure diagnostic, never as a debug line or
  outcome change. Never publish configuration responses; overrides, configured policy,
  cancellation and unconfirmed cleanup are not fallbacks.
- The local target registry initializes `platform` at `https://api.app.zeroshot.sh` with a persistent hosted device identity.
- Local runs select an optional `--cwd` workspace, defaulting to the invoking folder. File arguments
  stay relative to the invoking folder. A Git checkout uses its canonical root; plain, unborn, and
  detached folders are allowed when the graph has no Git delivery. Workspace and active Zeroshot
  state roots must be disjoint, including symlink aliases and missing state suffixes. Recovery
  reuses the stored workspace; local idempotency cannot return a run from a different folder.
  New local Git-delivery submissions require a clean index and worktree, including unignored
  untracked files, after idempotent-replay lookup and before controller launch. Use the captured
  native Git environment so admission and staging share ignore rules. Pending merges and moved
  or staged gitlinks block admission; non-stageable contents inside registered submodules do not.
  Delivery workspace-state and trusted-validation checks use the same root-stageable dirt rules.
  Git's successful staging warnings reach run output after terminal review observation and before
  reconciliation; unregistered embedded repositories warn
  without being blocked. Runs without Git delivery and retained-workspace resumes are unchanged.
  Workspace leases hold shared ancestor locks and an exclusive selected-root lock, so overlapping
  folders exclude each other while sibling folders remain concurrent. Lease guards explicitly
  unlock on drop so fork-before-exec children cannot retain released ownership. Git metadata and
  transport commands discard ambient repository-selection variables before inspecting the selected folder.
- Local delivery commits on HEAD and publishes that same attached branch, including earlier
  unpushed commits. Read attached branch names from full `refs/heads/` symbolic refs; abbreviated
  names change when a tag shares the branch name. In-place runs never create managed run refs.
  PR/merge targets a different upstream branch or the remote default, and refuses the target
  branch before agent launch with
  feature-branch/`--worktree` guidance. Explicit push may publish any branch. Never force-push;
  non-fast-forward and policy refusals stop without repair. `--worktree` prepares a retained Git
  worktree on a managed run branch before controller startup for all graphs/harnesses. Its derived
  storage root must be canonically disjoint from the invoking checkout before directory preparation
  or worktree creation. Its local
  base is exact invoking HEAD; explicit remote base fetches the upstream and requires one. Its
  review target remains the invoking upstream/name. Copy no untracked resources and perform no
  dependency setup. Recovery metadata retains the chosen source commit, workspace, invoking
  workspace and attached branch; local request fingerprints preserve replay before remote I/O.
  Resume reuses that workspace and attaches its branch without rewinding shared refs, refusing
  attachment when another registered checkout owns the branch.
  Shutdown-aware controller startup forwards the recorded branch and review identity into
  candidate construction, including recovery after an interrupted checkpoint restore.
  Legacy local Git-delivery recovery without recorded branch ownership refuses before checkpoint
  restore, successor publication or controller launch; preserve the workspace and start a new run
  from the intended attached branch rather than selecting contained delivery's managed fallback.
  Detached-HEAD guidance prefers a unique retained workspace branch, verified against local refs,
  ancestry and worktree ownership; checkout history is a fallback, never a guessed default branch.
  Publishing already committed work may confirm the admitted source commit without a new commit,
  including unchanged local and remote HEAD after resume.
  Exact clean merged-head recovery accepts that same local source commit, with inclusive ancestry.
  Merge confirmation during handoff also requires a clean workspace at the exact reviewed head
  with admitted source ancestry and the configured local branch still attached. A saved review
  binding never confirms unpublished local work or overrides an attached-branch refusal.
  Unbound local review discovery and creation select active reviews rather than branch history,
  including resumes that have not recorded any delivery;
  trusted delivery records a confirmed review binding in the existing recovery metadata before
  target reconciliation, polling or handing off, so repair, timeout, cancellation and crash preserve
  that identity. Private recovery evidence distinguishes observed identity from completed first
  publication; saving identity never skips target reconciliation. Successors
  recover it from that metadata or older trusted lineage ledger executions (including checkpoint
  seeds) and carry it through the private bootstrap, retaining terminal
  safeguards for that review without adopting branch history. Source-equal, already committed local
  candidates can integrate a diverged target and return through review; contained runs retain their
  mutation requirement. Git integration pins
  delivery identity per command and never writes it into shared worktree configuration.
  Existing attached HEAD survives checkpoint restore while files/index restore independently. Local
  checkpoint restore requires `--discard-workspace-changes` and cannot recover rewritten ancestry:
  merge the fetched published head into the run branch first, or start a new feature branch/run
  from that head; never rewind shared refs to make a checkpoint deliverable. Worktree
  removal is explicit; hosted/named-target managed branch behavior is unchanged. These flags
  are local CLI state and do not change MCP tools, protocol source types or remote plugins.
- New local PR/merge submissions verify the remote target branch before controller startup,
  after checking for an existing submission. Remote I/O stays outside the shared submission lock;
  admission rechecks idempotency before launch. Preflight reuses the prepared native snapshot and
  delivery binding's resolved credentials; native Git starts in the selected workspace so relative
  search paths resolve consistently. Shared delivery credential helpers preserve declared
  GitHub token precedence and source-only Git authority; forge API tokens remain provider-owned.
  Local PR/merge submissions and resumes also perform one authenticated, read-only forge project
  and server-version check before controller launch. A plain resume makes it under its recovery
  claim with the shared submission lock released, then rechecks the retained workspace record
  under that lock before it publishes the successor. Checkpoint resumes defer that single check
  until restoration under the controller's workspace lease, before runtime construction and agents.
  The deferred check observes the controller's stop request: the GitHub project read runs in the
  shared stop-aware capture, which ends the `gh` process tree with confirmed cleanup, the GitLab
  request is dropped, and a stopped check never admits the run.
  The source adapter factory shares the ten
  proxy/CA fields in `source_code/adapters/core/network` between that check and delivery, filtering
  the same private caller snapshot used by Git; the controller OS environment remains minimal.
  Capture anchors relative CA paths to the invoking directory once, so worktree creation and resume
  cannot reinterpret them. Empty uppercase proxy/bypass values fall back to populated lowercase values.
  Preflight reuses delivery's `source_target` repository selection; caller source IDs may differ
  from forge repository IDs. GitLab applies captured HTTP/HTTPS/SOCKS proxies, bypasses and PEM trust paths explicitly without changing the
  process environment. GitHub forwards only those network fields to its isolated `gh` child;
  on macOS and Windows `gh` honors the CA paths only when built with Go 1.27 or newer and
  otherwise uses system certificate trust. Proxy URL credentials are redacted
  before diagnostic bounds and remain private in Git, forge and final delivery feedback.
  Fresh review delivery reconciles the target and returns changed candidates through review before
  its first publication, including when the local feature branch already exists on the remote.
- Git checkout and push use credential-free HTTPS or SSH locators independently of a forge.
  The target image includes OpenSSH and a fixed root-owned libnss_wrapper for pinned SSH
  transport. Contained SSH Git uses private per-command user/group records for allocated numeric
  identities; local Git keeps native identity lookup. Contained runs still require an
  exact committed Git source. Explicit source connections
  authorize only transport fields; these values never enter agent environments or durable source
  records. Source-less local candidates construct no delivery adapter.
- Resumed Git delivery accepts intentional run-branch commits and reuses an identical complete
  staged candidate tree without recommitting when no merge is pending. Pending merges retain the
  existing reconciliation path; different local work still requires review. Review
  discovery uses the source repository/run branch before target validation, refusing retargeted or
  ambiguous reviews. GitLab rechecks MR/ref lag twice locally during observation and post-push
  metadata synchronization, then reports persistent disagreement as a terminal identity error.
- `source_code_provider` owns effect-free source and delivery contracts and one provider registry.
  Maintained implementations live in `source_code/adapters/core`; contributed implementations
  live in `source_code/adapters/contributed`. Git is shared transport; forge adapters supply review,
  checks, and merge behavior through the existing delivery nodes and opaque repository/review IDs.
  GitHub and GitLab are maintained forges. `SourceForgeIdentity.baseUrl` is a canonical credential-free
  HTTPS web base, including an optional installation path prefix. Review/merge identity must match
  the Git remote before credential resolution. GitHub Enterprise Server requires 3.22.2 or newer;
  GitLab requires 19.5.1 or newer. Adapters check versions and refuse older or unknown servers.
  Native `gh`/`glab` login resolution is host-scoped and fills only wholly missing delivery connections;
  explicit or stored values win, and partial values fail. Ambient tokens require a matching
  `GH_HOST`/`GITLAB_HOST`, defaulting only to GitHub.com/GitLab.com when absent. Unknown hosts require native configured
  identity or paired `--source-provider`/`--source-base-url` overrides. Shared Git transport remains
  independent of forge API credentials. GitLab uses bounded HTTP effects and synchronous SHA-guarded
  merges; required merge trains remain pending. A merged fast-forward MR without explicit merge
  or squash SHAs confirms its bound reviewed head only when the validated target equals that head
  or their captured-SHA merge base proves the head is a target ancestor. Other merge methods and
  invalid explicit SHAs never fall back to the candidate. GitLab's complete HTTP 400 missing-source-branch
  creation validation retries only within the existing review-sync batch, rechecking the published
  candidate before each POST; uncertain creation responses retain their at-most-once fence.
  `need_rebase` merges the captured target into a
  workspace descending from the MR head, bounded by their fork point rather than the MR head, then
  returns repair; a candidate already containing the target is refused naming the merge method and
  target revision. GitLab readiness, approval handoff and merge share
  required-CI policy: explicit `only_allow_merge_if_pipeline_succeeds: false` yields `NotRequired`
  regardless of head-pipeline presence or status, never `Passed`. Required or unknown policy needs
  captured-head CI evidence; absent/running CI stays pending and failure/skipped rules still apply.
  Synthetic pipeline SHAs do not prove required captured-head CI. All non-CI gates remain authoritative.
  Failed required GitLab pipelines include REST and GraphQL configuration errors, failed regular
  and trigger job names/stages/reasons, and labelled, credential-redacted trace tails. GraphQL
  error-message reads bind the captured project/pipeline/commit and use bounded cursor pagination.
  Both forges share CI diagnostic bounds and excerpt formatting in
  `source_code/adapters/core/ci_diagnostics`; unavailable traces retain the job list.
  Empty generated delivery bindings acquire the adapter's default connection only after source or
  comment destination selection. Managed GitHub.com token/file fallback never authorizes another instance.
  Adapters own optional canonical review/commit receipt links and comment permalink validation.
  Provider-owned comment normalization runs before the publisher hashes and records its durable
  intent; canonicalized text must retain one exact publication marker.
  GitLab owns idempotent literalization of slash-leading generated prose and ready, nonempty MR
  titles bounded to 255 Unicode characters. Its same title normalizer supplies generated delivery
  commit messages before preflight reconciliation or commit creation, preventing push-triggered drafts.
  Shortened titles retain the original once inside the
  generated description section. Existing GitLab reviews without either generated-body marker
  acquire an appended generated section, preserving human text; partial, duplicated or reordered
  markers still refuse metadata refresh. Refresh preserves surrounding human text and later
  human-applied draft status; comment literalization precedes hashing and durable publication intent.
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
  provenance. Terminal metadata exposes the confirmed native output as `deliveryReceipt`, independently of graph output. Generic local comment graphs need no
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
  are successful reviews. Selecting a review template defaults to comment
  delivery. PR and code review currently require comment delivery for comparison preparation; issue review
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
- `builtin.request.wait@1` is a host-run verifier with the fixed wait contract and a fieldless
  `request` binding. It permits seq/choice/loop placement, one attempt and no instructions or
  timeout; par/map ancestry and stack profiles refuse it. Exactly one RFC 3339 `dueAt` or integer
  `durationMs` determines its deadline, bounded to 30 days. Wall-clock checks occur at least once
  per minute. Local/direct hosts wait in place by default. A host-only in-place threshold or a
  suspension request defers a wait until due, without a successful node outcome or Active Memory
  postmortem. Its conditional continuation seed settles the wait only after the due-time check.
  Suspension ends the controller only after checkpoint publication and runtime cleanup. Its
  successor uses graph state without restoring local/direct workspace bytes. Vector entries live
  in the ledger and retain identities and order across every successor.
  Hosts with their own workspace storage opt in through one suspension policy and publish the
  complete graph history and vector record before suspension cleanup. Direct targets enable it.
  Discovery uses separate strict `{kind}` markers: `request_wait` (`openengine.request-wait/v1`),
  `run_suspension` (`openengine.run-suspension/v1`), and `run_vectors` (`openengine.run-vectors/v1`).
  Missing or wrong kinds refuse their operations after discovery and before token/body/session;
  extra marker fields fail discovery while unknown extension keys remain ignored.
- `code-review` reuses the `pr-review` input type and review continuation, so `--task-from`,
  comparison preparation and comment publication are unchanged. Triage returns a summary and at most
  six units (maps allow eight; overflow fails). Per-unit maps run two broad, a depth and a design
  reviewer beside five whole-change hunters (logic, state, contracts, errors, claims); the claims
  hunter checks linked issue acceptance criteria. Per-unit and remaining-file finalizers merge, try to
  disprove and rewrite `{file,startLine,endLine,severity,title,body}` findings, and a report agent
  renders one Markdown comment. Every agent is a read-only verifier with two attempts and no deadline.
  One choice per agent or map keeps each guard's control space under the verifier ceiling; any
  agent error or map overflow fails before publication. Instructions concatenate fixed `include_str!` fragments at compile time; there is no runtime
  prompt parser. Inspectors do not install dependencies or start applications, services or browsers.
  The report stays under 12,000 characters and 48,000 UTF-8 bytes to leave room for the publisher header. The supplied runtime splits agents across Claude Code and Codex;
  model choices remain caller-authored. Its supplied local runtime sets `workspaceCheckpoints: false`
  to skip file snapshots and Restic while preserving durable history and comment receipts. Uniform
  runtimes can select the same policy; named targets require the field enabled or omitted.
- Built-in template selection defaults to PR delivery for `software-change`, comment delivery for
  `pr-review`, `code-review` and `issue-review`, and no delivery for the other templates. Explicit supported delivery choices override
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
  hosted operations, including stack discovery routes. Reuse requires the same target, login,
  OAuth endpoints/client, and audience with more than thirty seconds of token lifetime remaining.
  Cache misses and login serialize under the shared cache lock before the cross-process refresh-family
  lock; tokens enter the cache only after session verification and rotated refresh-token persistence.
  Login clears the slot before attempting authentication. HTTP authentication rejection invalidates
  only the exact cached issuance, never a newer concurrent token; only stacks retain their
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
  `inputTokens` means uncached input; input, output, cache reads and cache writes are disjoint.
  Adapters normalize native counters before publication: Codex subtracts cache reads and writes
  after its resumed-session delta while retaining native baselines; Copilot subtracts both per call.
  Recorded usage remains unchanged and omitted cache counters remain unknown.
  `TokenUsageDelta::checked_add` owns bounded provider-neutral usage arithmetic. Overflow or
  malformed claimed Pi usage makes the turn total incomplete permanently rather than publishing
  an earlier partial total. Native
  authentication merges suppress all ambient forms of a credential group when a connection
  declares one form. Provider
  documents use atomic private-file replacement with descriptor-based ownership; Linux pins the
  private home before publication and never follows a worker-created destination link.
- Structured-output correction runs in the same provider session under the node's existing
  execution policy; local response validation remains authoritative. Correction turns name a
  detected envelope fault (empty reply, text before the object, unterminated object, or content
  after it) and restate the expected response as a one-line skeleton whose placeholders are not
  JSON; parsing stays strict and nothing is repaired or extracted.
- Provider continuation is bounded: Claude continues once after a retryable `system/api_retry`; Codex continues
  once after a terminal execution error. Both send literal `Continue` in the same session when one
  exists. A rejected credential or unavailable model with a native machine-readable signal is
  `refusal` with `authentication_required` or `model_unavailable`, bypasses continuation and node
  attempts, and skips Active Memory postmortem when it is the inherited source node's final outcome.
  Claude's authentication `api_retry` does not authorize continuation. Force stops use
  `refusal`/`stopped`; cut or malformed streams stay retryable `crash`. Classification belongs to
  each adapter and survives capsule transport; never infer it from provider message wording.
  Claude's native rejected `rate_limit_event` followed by an explicitly failed native result uses the same typed
  rejection plumbing as `refusal`/`declared_failure`, with reset details and manual-resume guidance.
  Missing, wrongly typed or unrecognized result subtypes cannot confirm quota exhaustion;
  their existing failure handling stays in place.
  Successful results and subsequent allowed events clear that quota evidence. Quota exhaustion
  bypasses continuation and crash attempts without changing authored recovery or postmortem policy.
  Codex exec message-only usage errors remain crashes until a native structured signal exists.
  Terminalization helpers preserve the complete worker outcome: live and reconstructed force stops
  use `stopped`, while runtime loss stays `crash`.
  Postmortem source outcomes use canonical durable history, including restored checkpoint
  prerequisites. Typed provider diagnostics use the session's current redactions after credential
  refresh, before leaving the adapter.
  Structured output receives at most two correction turns before `malformed`.
  A Codex `turn.completed` with a final agent message preserves its answer and usage across later
  process failures. Its remaining output and process wait share a 250 ms grace, followed by bounded
  release and confirmed process-tree cleanup. Teardown failures are logged once without replacing
  that answer; incomplete input, output delivery failures, cancellation and unconfirmed cleanup
  retain their failure semantics. Reusable Codex session files survive this per-turn release.
- Pi drives one `pi --mode json` process per turn, with the node prompt on stdin so a large input
  payload can never exceed the platform argument limit. Model identifiers stay caller-owned and Pi
  owns their resolution. Its provider lanes are `anthropic`, `openai`, `openrouter`, `bedrock`,
  `github`, and `gateway`; the gateway lane synthesizes a private `models.json` that registers its own Pi provider
  against the caller's base URL and references the declared key by interpolation, so no secret enters
  the file and no Zeroshot-owned model catalog exists. It registers its own provider rather than
  retargeting a built-in one because Pi honors `api`, `apiKey`, and `baseUrl` on a registered
  provider while ignoring the first two on a built-in one; that is what lets a declared `GATEWAY_API`
  name the wire protocol (`openai-responses`, `openai-completions`, or `anthropic-messages`) and
  reach any endpoint Pi implements without probing it. The registered model entry carries the
  caller's identifier, the chosen protocol, and a `reasoning` flag mirroring the binding's authored
  effort. It carries `contextWindow` and `maxTokens` only when the caller declared them, so omitted
  limits and cost stay Pi-owned defaults. The same file is Pi's only
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
  The `github` lane maps to native `github-copilot`; a declared GitHub user token seeds a private
  native OAuth entry, never raw model authentication. Pi owns exchange and refresh, and the raw
  declared token is removed from the process/tool environment. Native-login runs keep their agent
  directory. Pi home overrides resolve native tilde shorthand against the captured user's home before
  detachment and leave `file://` decoding to Pi. A native-local lane reuses
  the user's stored Pi login from its agent directory; every other lane receives a private agent
  directory so no ambient configuration, catalog, or credential is inherited. Session identity is a
  bounded SHA-256 of the node-instance or execution identity, because Pi accepts
  `[A-Za-z0-9._-]` with an alphanumeric first and last character and imposes no other bound. Pi reports a failed or aborted model response in its event
  stream while still exiting successfully, so the terminal event's stop reason decides provider
  success and exit status alone is insufficient. Pi exposes no response-schema flag, so the Pi lane
  carries the machine-readable provider schema in the prompt rather than the `NodeResponseContract`
  object, and correction turns restate that same schema so the retries converge. The schema dialect
  follows the lane's wire protocol: the `anthropic`, `bedrock`, and `github` lanes and a gateway
  `anthropic-messages` protocol use the neutral schema, while the `openai` and `openrouter` lanes
  and the gateway `openai-responses`/`openai-completions` protocols use OpenAI strict mode; a
  gateway lane must name its protocol through `GATEWAY_API`. Local validation remains authoritative
  and corrections stay bounded to two turns.
- Copilot uses the pinned CLI's headless JSON-RPC protocol 3 and caller-owned model IDs. Its
  `github` lane retains native login; explicit OpenAI, Anthropic and OpenRouter lanes use native
  BYOK; gateway selects its native API explicitly. Bedrock uses native BYOK Responses with the
  regional AWS OpenAI-compatible endpoint and bearer credential, without a signing shim.
  Canonical local credentials and endpoints stay private, behind declared values. Endpoint aliases
  share precedence; switching endpoints clears stored headers before applying declared ones. Remote session synchronization is disabled with `--sync-session off`.
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
  `provider/id` remains the unchanged process-level registry selection. Contained GitHub lanes require
  `COPILOT_GITHUB_TOKEN`; BYOK lanes require their canonical provider fields. Optional
  `COPILOT_GITHUB_TOKEN_EXPIRES_AT` is Unix seconds;
  expiring credentials use the runtime resolver callback, validate the configured GitHub host, and
  must retain more than one hour. Present but empty or malformed declared credentials fail closed
  instead of falling back to native login.
  Grammar compilation rate limits retry at most three times with exponential backoff and
  per-session staggering after confirmed cleanup and a successful event drain. Retries resume
  the same session with its current correction prompt and host context; other provider errors
  remain terminal.
  Copilot RPC bounds each message to 64 MiB and bounds pending requests and output queues.
  After an accepted answer, a valid `session.detach` refusal or peer EOF is logged and keeps the
  outcome, including a closed stdin pipe during that request. Malformed RPC replies/events, other
  RPC writes, cancellation, durable output and cleanup failures still fail closed.
- Provider JSONL readers do not cap cumulative output. They share the 64 MiB unfinished-record
  guard, accept a complete final record without a newline, ignore unknown future event types before
  validating provider-owned fields, and continue draining after the first valid terminal event
  (Codex bounds that drain after a completed turn).
  Their shared decoder replaces invalid UTF-8 and unpaired Unicode surrogate escapes with U+FFFD
  before adapter parsing and redaction; all other malformed JSON retains each reader's rejection policy.
- Adapter registrations own CLI version ranges, native version arguments/parsers, local environment
  allowlists and upgrade steps. Local runs and checkpoint resumes probe every distinct lane executable
  after workspace restoration and before runtime composition/admission; ACP probes before creating a
  session. Shared version probes use the existing process runner, captured search path and native
  environment, a 60-second deadline and bounded output, including npm Windows command shims.
  Controller shutdown cancels in-flight probes and awaits confirmed process-tree cleanup before
  returning the startup failure; a cancelled probe cannot admit even a valid reported version.
  Truncated version output remains available only for classification; omit it from diagnostics so
  credentials crossing the byte limit cannot escape exact-match redaction, including Claude's first-turn check.
  Unknown versions fail closed. Minimums are Claude 2.1.310, Codex 0.155.0, Copilot 1.0.70,
  Pi 1.4.0 and Hermes 0.22.5. Hermes 0.22.6 is the exclusive upper bound because native agent cleanup
  is incompatible; all other upper bounds remain open. OpenCode has no range until the version 2 port.
  Contained Claude sets `DISABLE_AUTOUPDATER=1` after environment merges to preserve image pins.
- Claude reads its adapter registration's range before each run's first model turn, running the
  selected executable with `--version` through that turn's launch path and environment. One `--print` process can write several results while background subagents or
  shells finish, so Claude parses every record through stdout EOF and the last result decides the
  attempt: a later error fails it, missing final structured output uses the bounded correction, and
  `subagent_stats.killed.system` above zero (subagents Claude Code stopped itself) fails it. Usage
  is the sum of each result's `model_usage_since_last_result` plus streamed message usage after the
  last result; an absent or malformed value leaves it unknown. Session-total `modelUsage` and
  main-thread `usage` never stand in. Each result's `permission_denials` lists that turn's denied
  calls; the transcript reports calls no earlier result listed, by `tool_use_id`, as one redacted,
  bounded error-stream record of tool names and counts without a failure diagnostic. It never
  reads `tool_input`, and denials never change the attempt; verifiers judge completion.
- Provider stdin and stdout are concurrent and bounded so large prompts and early output cannot
  deadlock. Incomplete stdin is fatal, while parsed identity, usage, retry, and diagnostics survive
  either I/O completion order.
- Durable provider events cross bounded async queues with backpressure. Cancellation preserves token
  usage and event order; overflow is explicit and produces an incomplete marker rather than silent
  loss.
- SQLite operations acquire the single connection asynchronously before running on a blocking
  thread; cancelling a caller does not release an in-flight database operation's ownership.
  Read-only ledger opens recover hot rollback journals only in a private image copied under
  SQLite's native shared read lock, with no reserved writer. SQLite's VFS supplies both the
  lock and database bytes, preserving native and same-process lock bookkeeping. The image
  excludes WAL and foreign super-journal references, reopens read-only after rollback, and
  shares the connection's lifetime through clones and cancelled blocking operations. Inspection
  never rolls back the retained ledger. Explicit local resume prepares its status through the
  control boundary; resume and force-stop can confirm the original lease and recover even when
  read-only access or temporary image creation failed.
  The connection is granted in request order, also to a pending request that is no longer
  polled, which then blocks every later operation: drop such a request before awaiting other
  ledger work in its place. The local controller's serving loop drops its terminal wait before
  it runs a requested stop.
- Live force-stop signals owned work before waiting for persistence. Local stop intent prevents
  interrupted work from becoming a retryable crash. Cleanup and durable output still precede
  final settlement. Confirmed runtime failure is observable before its persistence attempt;
  a later durable terminal snapshot remains authoritative over the in-memory fallback. A local
  controller keeps serving until that attempt settles, bounded beyond SQLite's busy timeout, so
  process exit cannot discard the terminal; success exits are never delayed.
  Detached local controllers latch Unix SIGTERM before bootstrap consumption and reuse this stop
  path, retaining workspace/checkpoint recovery data. Retained delivery receipts survive this shutdown
  even with workspace snapshots disabled. One process deadline bounds preparation,
  cleanup, persistence and runtime exit to 30 seconds after the first request; repeated signals
  cannot restart it. Startup review fetches and checkpoint capture/backup/extraction observe this
  request and confirm subprocess cleanup before releasing workspace ownership. Snapshot traversal
  reuses cancellable file copying and checks cancellation while validating and syncing metadata;
  all snapshot Git commands use the shared bounded, cancellable process runner. Blocking snapshot
  work bridges to its owning async runtime; synchronous callers use a joined private I/O thread.
  Restore prevalidation is cancellable and supervised; replacement finishes once it starts. Only a
  restore with a stop request, which the local controller awaits to its end, replaces the
  workspace on the blocking pool. Every other restore replaces it inside its own poll, so a
  dropped hosted allocation leaves no writer behind. Only confirmed
  checkpoint cancellation settles as a stop; cancellation during a final checkpoint preserves the
  latched primary outcome. No terminal is invented when settlement fails or
  the deadline/OS ends the process. Terminal closure remains detached. Windows keeps Ctrl-C and
  Ctrl-Break guards and adds no close/logoff/shutdown handler; cooperative stop uses force-stop.
- Failed and suspended local and direct-target runs retain an exclusively claimable workspace for a successor
  attempt. Local recovery metadata lives beside each run ledger while the checkout remains
  user-owned; direct targets retain only the candidate and Git metadata, dispose private runtime
  state after confirmed process cleanup, quarantine retained trees under supervisor ownership, and
  recursively transfer them to the successor's newly leased writer identity before admission. They
  advertise `openengine.workspace-recovery/v1`. Local resume persists predecessor/successor
  lineage before controller launch, uses a process-held file lock during admission, and reconciles
  an interrupted launch before permitting another successor. A launch whose controller is confirmed
  stopped is reconciled without a handoff wait. Status reports a claim whose launcher released it
  and whose controller neither runs nor admitted the run as recoverable, so a crash cannot strand
  the workspace. Local force-stop holds the same per-run recovery claim before reading status or
  successor ownership and through the controller reply, including when the run is still running.
  Direct-target retained-workspace handoff records both sides before moving the tree
  and reconciles incomplete handoffs at startup. Resume reserves this durable handoff before
  acknowledging the successor; asynchronous credential resolution and preparation reuse that claim.
  Live-engine force-stop shares a per-run control turn with resume and checks successor ownership
  before signalling the engine, so a stale suspended engine cannot stop an accepted continuation.
  Unrelated submissions and resume reads for an ineligible live source cannot block cancellation.
  Direct-target status for failed and suspended runs exposes the immutable admitted connection requirements so the CLI resolves
  fresh resume values without consulting changed profiles. Recovery lineage also retains the root
  attempt's delivery identity so every successor reuses the same delivery branch and pull request.
  Successor composition explicitly authorizes a fresh delivery adapter to adopt that lineage-owned
  branch; ordinary fresh adapters still reject unexplained existing run branches.
- Runtime plans accept `workspaceCheckpoints`, defaulting to true and omitted from the wire at
  that default. Local opt-out leaves the existing executor and durable ledger intact, skips Restic
  construction and snapshot cleanup, and refuses selected checkpoint restoration before credential
  resolution. Named/contained targets currently refuse opt-out explicitly. Decision bindings are
  not workspace writers. Executable steps remain conservative potential writers.
- `openengine.workspace-checkpoints/v1` adds `run/checkpoints` and selected checkpoint resume to
  retained workspace recovery. Default resume of a failed run starts a fresh graph on the retained workspace;
  checkpoint resume restores a private input snapshot and settled prerequisite executions. Neither
  imports old provider sessions, secrets, or token usage. Suspended runs accept only default resume,
  continuing at the held-back node, or after a due wait. Every successor inherits the ordered vector
  record. Continuation restores no file locally or on a direct target and accepts no discard flag. Checkpoint IDs never become filesystem paths.
  The reducer owns boundaries: an outer parallel or mapped group is one unit across all nested work
  and dispatch waves. The supervisor captures only after all live executions and cleanup have settled;
  read-only boundaries reuse bytes. Local/direct storage normalizes Git into a temporary private
  filesystem stage, commits it to one deduplicated Restic repository per recovery lineage, and then
  removes the full stage. Small catalogs atomically map logical checkpoint IDs to Restic snapshots.
  Restic drains progress and private stderr completely with bounded memory, retaining only the
  latest summary under a per-record size bound. Errors identify the operation and failure stage
  or exit status without private child output; failed backups never replace a published checkpoint.
  Versioned execution seeds live in separate files and are read only for selected continuation.
  Target restart restores the latest published snapshot without importing a seed. Local restart
  restores nothing: the user-owned checkout keeps its files, index, HEAD and branch, so work done
  after a failure enters the successor. A selected local checkpoint is an explicit rollback that
  requires the CLI-only `--discard-workspace-changes` (rejected with `--target`); without it resume
  fails before credential resolution, successor claim or workspace change and names the opt-in
  command. Rollback keeps the shared-Git restore contract below. Restore staging is a
  private sibling of the workspace so final replacement stays on one filesystem. A restore reads
  the selected seed and extracts and validates its complete stage before changing the workspace.
  Restic runs in a contained process tree that is confirmed stopped on failure or at the deadline.
  Git objects and private Git files such as HEAD and the index are published by rename, so
  repeating the restore after an interruption at any step installs the complete snapshot; a hard
  crash during replacement can still leave a partial checkout until then. Local resume pins the
  selected snapshot and records it with the successor as the attempt in the predecessor's recovery
  metadata before launch, and clears it once that successor is ready. The local stage is named
  after its attempt, so the next resume removes only that stage. The same selection must still
  resolve to the pinned snapshot; another consented checkpoint selection replaces the record, and a
  plain resume is refused while it remains. A `Latest` record left by an older plain resume is
  completed only with consent: `resume RUN_ID --discard-workspace-changes` replays its pinned
  snapshot. Outside that case the flag requires `--from-checkpoint`. Successful lineages
  delete their catalogs and repository only after durable terminal success; failed and suspended lineages retain
  both for resume. Disposable checkpoint staging cleanup is best effort after confirmed
  runtime cleanup and before observer ledger access. Copied read-only directories are made writable
  only inside private scratch; staging cleanup failure cannot block terminal truth or status.
  Hosted failed runs likewise collect checkpoint staging as best effort after process cleanup;
  retained recovery still requires runtime cleanup, workspace ownership transfer, and durable
  recovery metadata. Force-stop and explicit discard retain authoritative lineage deletion.
  Local force-stop stays destructive for the whole lineage and retains no checkpoints: after
  confirmed runtime cleanup it reuses `discard_lineage` to remove every catalog naming the shared
  repository, predecessor and successor alike, before the repository. A catalog removal failure
  keeps the repository and reports cleanup unavailable. Other lineages are untouched.
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
  Vector pagination resolves entry IDs through a per-run ledger index, including inherited IDs,
  then replays after that cursor instead of reading earlier pages again.
  Local CLI inspection opens finished runs directly from read-only ledgers. Vector list and show
  use the same retained-storage checks and keep the validated run directory open for the read.
  Unfinished local CLI and UI history readers share the bounded portable-controller availability
  probe: original lease identity bound in readiness, read-only shared-lock probes, endpoint response
  and process evidence.
  Missing/inaccessible/replaced evidence and live or reused PIDs remain unknown; only an absent
  process with its original released lease establishes lost. Availability is optional controller
  metadata with observedAt, never a durable status, terminal outcome or inferred death time.
  Inspection never binds a controller, claims recovery, repairs permissions or rewrites readiness.
  When the recorded read fails, inspection still returns a responding controller's status and
  follows its streams; without that answer the read error stands.
  Recorded reads exit 0; unavailable watch/logs/attach replay through their captured durable head
  and exit 1 naming resume and force-stop. Only those controls confirm original lease ownership
  and process absence before recording runtime_lost. Finished stream exit semantics stay unchanged.
  Control fast paths require a terminal recorded in the ledger; a controller's unpersisted
  runtime_failed projection does not bypass force cleanup or resume ownership checks.
  A force request goes to any endpoint that accepts the connection for its readiness identity,
  without a status reply first, so a controller whose ledger is busy still stops owned work. An
  endpoint that accepts nothing is taken over only after confirmed loss.
  Loss recovery retains the original readiness evidence until reconciliation is durable, so
  failed or interrupted persistence cannot strand subsequent resume or force-stop attempts.
  A finished or suspended snapshot closes a subscription only after replay reaches its durable cursor.
  A retiring local controller closes connection writers and joins their request cleanup before
  ending its runtime. EOF remains a reconnect signal; durable observation resumes from the last
  emitted cursor instead of reporting a storage failure caused by runtime shutdown.
- Bulk replay uses the WebSocket client's opt-in subscription backpressure on a dedicated
  connection; control requests and ordinary observers keep their independent connections.
- Safe-log timestamps are captured at the producer boundary as positive JavaScript-safe Unix epoch
  milliseconds and remain unchanged across durable replay.
- Each run records its executing Zeroshot version before preparation or dispatch logs; resumed
  successors record their own build. The supervisor serializes version initialization across drive
  and force-stop. Force-stop stops live work before version logging and request persistence, and
  skips ledger reads once version initialization completes.
  Adapters log accepted CLI versions through the existing output
  bridge once per run and lane when they read them; Claude serializes its initial version check
  for each run even when ACP reuses the adapter. After invocations drain, `NodeDriver::close_run`
  releases that run's check state; lane routing forwards the hook to its adapters. ACP records its
  build at run creation before publishing the supervisor to cancellation.
- Runner start reserves an execution and returns its handle before asynchronous provider startup.
  Pending start waits are cancellable; accepted startup failures settle through the owned handle.
  Pending remote starts reread connection loss after a close notification so failed closure cannot
  race into an ordinary cancellation result.
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
  Shared process spawn errors include the program as an escaped preview of at most 256 bytes plus
  a truncation marker, alongside the OS cause; they never include arguments or environment values.
  These public summaries stay separate from private operator diagnostics.
  Existing Error output and safe-log records may carry `FailureDiagnosticKind` metadata; their text
  is the public-safe explanation. The ordinary ledger projection retains a bounded `FailureContext`
  and attaches it to failed terminal results in the same append transaction. Status, watch, SDK,
  shared UI and archived history carry that result directly. Known runtime causes remain distinct
  from recorded node failures; recovered/voided evidence never establishes terminal causality.
  Verifier adapters, including Git delivery, publish bounded graph-defined signals and diagnostics
  through the existing output/log bridge using their complete effective credential redactions (including
  captured native authentication), before truncation and SafeLog persistence. Failure context retains latest verifier
  results per node and map path separately from runtime causes and recorded execution errors.
  Only actual failure diagnostics suppress duplicate settlement errors; verifier results never do.
  The reducer supplies bounded exhausted-loop visits and completed round counts; retries never count
  as rounds. Settled unreachable parallel joins replay canonical branches without scheduling
  decisions; speculative probes and race/map losers never contribute loop evidence or consume its bound.
  Public execution and log projections expose zero-based outermost-first map indices,
  omitted outside maps, without changing opaque execution selectors.
  Diagnostic overflow marks context incomplete independently of token usage, even after retained
  exhausted-loop entries are removed by later convergence. Producer and capsule
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
- Linux local process-group commands use one private subreaper per command. The supervisor keeps
  only its lifetime socket and matching workspace lease descriptors above stdio, closes unrelated
  inherited descriptors, and kills/reaps its kernel-owned descendants after root exit or owner loss.
  Cleanup evidence and the original root status publish only after every descendant is reaped;
  abrupt controller death keeps workspace locks held until then. No PID registry, cgroup or root
  privilege is required; procfs provides owned-child enumeration. Dedicated worker identities and
  Windows Job Objects keep their existing containment. macOS process groups do not cover descendants
  that detach themselves; stronger macOS ownership remains deferred.
- Contained provider sessions clean up surviving descendants immediately after the main process exits,
  before draining inherited output streams. Buffered output still drains under any explicit command
  deadline and a ten-minute ceiling while observing cancellation; cleanup stays scoped to its session.
- Git delivery owns authenticated repository operations and observes the run PR and branch before
  staging. It pushes the captured candidate SHA, preserves published ancestry and local work, and
  recognizes confirmed remote success after a lost response. Authorized branch updates may advance
  the unchanged candidate directly; other integrated remote changes return `repair_required` so the
  authored graph decides what work follows. Agents receive local refs and conflicts without the
  delivery credential. Closed PRs, identity changes and lost published ancestry stop delivery.
  Failure diagnostics name the project, source/target branches and known review/head, including the
  last confirmed publication. Diagnostic publication evidence is separate from reconciliation and
  withdrawal authority: a repair push may advance it before review synchronization succeeds.
  Recovered review receipts seed diagnostics before fresh observations; their saved publication
  flag alone controls whether first-publication reconciliation has completed.
  GitLab merge HTTP 401 is a missing merge permission only when a fresh authenticated user read
  succeeds; rejected tokens retain authentication failure classification.
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
  Local Git retains the caller's OS identity, separately from delivery commit authorship.
- Delivery commits default to `Zeroshot <delivery@zeroshot.invalid>`. A hosted
  `TargetRunRequest.commitIdentity`, or `RunResumeParams.commitIdentity` for a successor, replaces
  that author and committer for every commit the attempt's delivery creates: prepared revisions,
  reconcile and target merges, and the contained repository-local identity pinned before conflict
  repair. Local delivery always keeps identity command-scoped and preserves shared Git configuration.
  Like resume credentials, hosts resupply it for each successor. `CommitIdentity` rejects values
  over 256 bytes, control characters, `<`, `>`, and values Git trims to nothing.
  `NativeV2CandidateDelivery::new` derives the shared transport's identity from the delivery
  config. Local runs keep the default and reject a resume identity. It travels with ephemeral
  source credentials, never run state or agent configuration; hosts decide which forges get one.
- An optional `commitCoAuthor` on the same requests is one more `CommitIdentity`, credited and
  never authoring, and resupplied for each successor. `SystemGit::prepare_revision`, where delivery
  commits the run's changes, credits it in that same single `git commit`, using
  `--cleanup=whitespace` and `--trailer "Co-authored-by: Name <email>"`, so a failed or
  interrupted commit never leaves those changes committed without the credit. Its `where=end`,
  `ifexists=addIfDifferent`, `ifmissing=add` and separator are pinned with `-c`, so Git ends the
  trailer block with the credit once despite workspace trailer defaults. No pin sets a `key`: Git
  would rename existing trailers whose names prefix it. The run's own trailers keep their names and
  values; Git only normalises the spacing around their separators. Workspace settings for the
  `Co-authored-by` trailer itself, such as a `key` or `cmd`, still apply, as other workspace Git
  configuration affects delivery Git. A message the pinned cleanup empties (only blank or
  `Signed-off-by: ` lines) gets no trailer, so Git refuses it instead of committing the credit as
  the whole message; without a co-author the commit command is unchanged. A conflicted merge that
  `prepare_revision` concludes after repair carries the run's resolution and is credited; merges
  Git completes itself (reconcile `merge --no-edit`, target integration) and the identity pinned
  for conflict repair never name it. It flows beside `commitIdentity` from source credentials to
  `NativeV2DeliveryConfig`, the shared transport, and workspace Git. Local resume rejects it, and
  requests without it are byte-identical.
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
- Named-target `run`, `resume`, and `stack submit` never read the invoking shell's `GH_TOKEN`
  without `--forward-gh-token`, neither as the legacy GitHub.com token, a declared connection field
  nor a direct target's `--task-from` review credential, so targets resolve GitHub access from
  their stored connections. Target authorities never read `GH_TOKEN` themselves; the CLI passes it. Each command reads every
  outbound value through one `GitHubTokenScope` environment. The flag fails closed when `GH_TOKEN`
  is unset, including under `--validate-only`. Direct-target resumes whose retained requirements
  need `GH_TOKEN` fail early without it. The local branch-tip lookup for an omitted `--revision`
  still reads `GH_TOKEN` and never sends it. Local controllers keep reading explicit `GH_TOKEN`.
  The Python SDK opts in with `Client(forward_github_token=True)`.
- Hosting embedders may install `HostedDeliveryValidationFactory` through
  `ProductionHostingConfig.delivery_validation`. Allocation supplies the exact admitted run,
  delivery lineage, workspace and process pool before agents start, only for Git-delivery graphs.
  The host selects a private executable outside the checkout; authored profiles cannot select it.
  Repository commands use the allocated writer containment, never the controller identity.
  `DeliveryValidationResource` retains host-owned auxiliary work and confirms its cleanup before
  delivery hands the workspace back to agents. Hook code and credentials remain host-owned.
  Clean candidates equal to the admitted base also reach validation before `NoMutation`; a hook
  may return repair without creating a commit, run branch or review, but cannot authorize an empty
  publication in a contained run. Local delivery may publish existing committed branch work;
  contained runs without a host validator preserve the ordinary no-mutation refusal.
- `ZEROSHOT_DELIVERY_CHECK_COMMAND` selects one private absolute executable outside the checkout
  for trusted local publication, review observation and merge validation. Its capability travels only through the consumed
  private bootstrap, never provider context or recovery metadata; resume selects it freshly. The
  controller owns process cleanup and passes public candidate facts without ambient credentials.
  Authoritative merged/closed observations settle before hooks or late feedback; a completed remote
  merge cannot become a new repair merely because main now contains the fix.
- Before first publication, delivery captures and fetches the exact current target revision and
  merges it while preserving candidate history and dirty work, except when GitHub authoritatively
  confirms a merge queue on the exact repository and configured target branch. That explicit deferred
  result preserves the reviewed candidate and baseline; target motion alone cannot delay publication.
  GitHub rechecks current queue policy at later gates. Without a queue, current base/head comparison
  routes missing target ancestry through ordinary authorized head reconciliation, including after
  recovery or queue removal with a nonstrict `CLEAN` status. No cached queue decision grants merge
  authority. Other forges and push delivery keep their existing behavior. Any locally changed candidate returns through
  the authored repair/review loop before push. Delivery feedback distinguishes immutable
  `sourceRevision` provenance from the captured `reviewBaseRevision`; verifiers and repairs preserve
  upstream changes. Unconfirmed baselines after external branch updates are explicitly unavailable.
- Delivery retries recognized transport failures within the caller's polling and cancellation
  policy, refreshing dynamic credentials once after authentication failure. Temporary credential
  resolution retries with backoff; confirmed refusal and malformed responses stop. Initial
  resolution belongs to each supervised execution, so it observes the authored deadline and does
  not block parallel dispatch. Pending authorized head adoption survives repair.
- Forge adapters authorize actionable review feedback before delivery checkpoints or repair handoff.
  Each inspection confirms stable author identity. GitHub accepts its API-authenticated `Bot`
  account type and stable numeric actor ID on the bound repository as app authorization when posted;
  app removal never invalidates that feedback and installation inventory is never queried. Human
  authors, including app actions attributed to a user, require fresh effective write/admin permission; GitLab uses active inherited or
  invited Developer-or-higher project membership and active account `state`. Enterprise Edition
  also requires explicit active `membership_state`; omission is valid only when instance metadata
  authoritatively confirms Community Edition, whose members/all endpoint excludes pending invites
  and access requests. Unknown or malformed membership status always fails the read. Cache author
  decisions only within that inspection; instance metadata never substitutes for fresh membership.
  Confirmed insufficient access or membership/permission HTTP 404 ignores feedback; failed,
  malformed or identity-mismatched lookups fail the complete read through delivery error handling.
  Association, authorship, requested-reviewer status, display names and login suffixes never authorize feedback.
  GitLab retains its membership rules without importing GitHub App semantics.
- Git diagnostics retain bounded, credential-redacted command/status/stdout/stderr details.
  Capture and native transport share credential-name selection for tokens, passwords and API keys,
  respecting operating-system case rules. Trusted Git commands pin author and committer identity
  after applying native transport context and keep automatic GC foreground on old and current Git.
  Local workspace inspection, staging, commits, and reconciliation share transport's private caller
  snapshot and native Git configuration; hosted Git retains isolated global/system configuration.
  Forge target integration, head adoption, and conflict commands reuse that configured transport;
  forge API subprocesses retain their separate isolated environment.
  Workspace Git and forge reconciliation parse private protocol stdout separately from printable
  diagnostics, including when native credentials collide with object IDs or Git-directory paths;
  parsing and failure classification must preserve raw evidence before credential redaction. Final delivery feedback
  also redacts the invocation's source credentials, its current refreshed issuance, and captured
  native Git credentials using the shared credential-name selector. GitLab object diagnostics redact
  credentials before bounded source errors are constructed; malformed object fields are described
  without interpolating their unbounded response text;
  unfamiliar failures use the existing optional `repair_required` signal. After exact-ref
  confirmation, only the exact forge-authored branch-protection and push-permission texts in
  `git_diagnostic.rs` (relayed `remote:` lines or GitHub's SSH `ERROR:` lines) become
  `policy_refusal`, never repair; generic hook declines, `[remote rejected]` and bare HTTP 403 stay
  on their existing paths. Only successful receipts
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
  exact `expectedHeadOid`; enabling auto-merge alone is not enqueue confirmation. Confirmed
  admissions retain their added-event identity until confirmed withdrawal. Polling never resubmits a
  removed entry: matching removal evidence binds the queued commit and complete required checks/job
  logs, returning CI feedback through the existing repair loop before another admission is requested.
  Completed transient submissions retry after fresh exact-head policy checks; retries retain earlier uncertainty so an
  empty snapshot cannot authorize repair. A complete GraphQL `UNPROCESSABLE` rejection rooted exactly
  at a null enqueue mutation adds no uncertainty; partial, nested, truncated, or unconfirmed responses
  remain unknown. Classification uses protocol bytes before credential redaction and never invents
  an HTTP status. Permanent forge policy failures remain execution errors. Before a merge
  delivery returns repair, refusal, error, or cancellation, trusted authority confirms withdrawal
  of queued/auto-merge effects or the exact already-merged revision. Failed withdrawal never
  authorizes agent repair. Authorities that schedule deferred merges must override
  `withdraw_merge_request`; its default is valid only when no autonomous merge effects exist. Merge commands retain contained process cleanup and use the GitHub API error boundary,
  never Git repair; changed gates cannot mask a permanent rejection. Configured
  branch-protection and active branch-ruleset contexts remain pending until they appear as required
  on the exact PR head. Supplement otherwise-ready observations with all pages of GitHub's effective
  branch-rules API, so a newly opened PR cannot look ready before its required workflow registers.
  Ruleset App bindings require matching check-suite App identity. Legacy status contexts satisfy
  unbound ruleset checks; unresolved App-bound checks remain pending. Missing or stale human
  review may satisfy PR readiness when aggregate ref-update policy positively requires approval,
  required checks have settled, and no observed conversation, linear-history, or signature blocker
  remains. Deployment requirements do not block PR handoff. Missing review-policy evidence stays
  pending, and this exception never grants merge authority. Outside merge
  queues, branch freshness advances only through an authorized compare-and-swap response. A
  reported conflict is routable only after the trusted lane fetches the exact current target and
  leaves a verified nonempty Git merge conflict in the workspace; repair agents receive no GitHub
  credential, and the trusted lane pins their contained repository-local commit identity to the delivery
  identity before handoff. Local checkouts retain their user's Git configuration. Merge receipts preserve GitHub's
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
  Every software-change delivery mode, including none, reserves remote publication for delivery;
  worker guidance forbids pushing branches or weakening forge restrictions, and both reviewers
  reject repairs that do either.
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
  Push publishes only the exact working branch revision. PR delivery stops only after required CI,
  freshness, and conflict checks pass; missing review approval is allowed and no merge is requested.
  PR and merge workers read all visible issue comments, review summaries, and inline comments behind
  one exact-head fence. A run checkpoint routes each new or edited item through the existing repair
  and verification loop once. Unresolved GitHub threads and GitLab discussions retain opaque IDs and
  content revisions independently of forwarding deduplication. Workers author typed `reviewResponses`
  (threadId, version, addressed/declined disposition and response); both existing reviewers independently
  verify them before the same delivery node posts and resolves only matching authorized conversations.
  Delivery rereads after replying and resolving; substantive edits/replies invalidate approval, and a
  resolution race reopens the changed conversation. Forge authorities retain exact pending reply
  receipts and approved revisions across interrupted calls; retries, feedback reads and merge admission
  reconcile them even when the conversation reports resolved. Confirmed revisions remain checked
  across later repairs independently of the latest `reviewResponses`, including an empty array.
  Delivery exports credential-free, provider-owned reply/revision receipts through a private runner
  event into execution history. Before posting a reply, the provider awaits its
  `SourceReviewCheckpoint` capability for a compact intent binding the authenticated delivery actor,
  unique response-body hash and approved revision. An uncertain POST reconciles only a unique exact
  actor/body match on that thread; absent or ambiguous evidence blocks delivery without repeating the
  POST or dispatching repair. A definitive HTTP 401 refusal durably retires the unposted intent
  before returning the original authentication error to the existing credential refresh/retry path;
  failed retirement acknowledgement blocks that retry. Normal publication acknowledges the returned reply ID before resolution;
  recovery may derive that ID from the already acknowledged intent. Settlement updates
  are acknowledged before delivery continues, including settlement or partial reply reconciliation
  during feedback inspection and merge admission. Those operations receive the same host checkpoint
  capability and stop if acknowledgement fails. Stage settlement and retirement changes, including
  reply ownership, until acknowledgement succeeds; a failed acknowledgement preserves the previous
  receipts even when the proposed update removes the final conversation. Enqueueing or reading an event is not acknowledgement:
  only the trusted supervisor acknowledges after ledger append, through the private capsule channel
  when applicable. Read-side reconciliation cannot resolve an unacknowledged in-memory receipt.
  Repeated identical acknowledgements reuse durable proof while polling; changed intermediate and
  final writes share monotonically increasing per-execution generations.
  Checkpoint seeds retain that opaque state; the supervisor restores
  the latest bound-authority state before Git-delivery forge reads, independently of graph output.
  Agents cannot supply these receipts through worker output or authored state. The existing checkpoint
  store also retains the latest host generation in the shared recovery-lineage repository, outside
  workspace and graph rollback. Disabling workspace snapshots keeps the metadata-only delivery
  store, without initializing Restic or copying the workspace. Receipts contain stable identities,
  approved revisions and response/reply hashes, never full response bodies; both adapters reserve
  serialized aggregate capacity before posting another reply, including earlier repair rounds. Restart keeps fresh graph history while restoring those owned facts;
  an older checkpoint cannot replace them, and conflicting generations fail closed. Custom checkpoint
  backends own the same lineage-retention contract through `RunCheckpointStore::delivery_state`
  and its receipt-only `retain_delivery_state` operation. Runtime failure retains acknowledged
  receipts after draining and before terminal publication. Successor admission reconciles the
  predecessor ledger before claiming or moving the workspace, including dead-controller recovery;
  hosted storage implements this same retention before allocation, independently of snapshots.
  Failed confirmation/reopening stays
  pending and blocks merge. Unchanged trusted replies are excluded from finding deduplication, while
  complete conversation revisions still include them. Replayed reviewed responses recheck resolved
  threads; if a reconstructed authority cannot prove a changed resolved revision, it reopens the exact
  conversation for reassessment using the existing graph checkpoint rather than accepting its flag.
  Unchanged unresolved conversations still block
  delivery; unauthorized participants require an explicit maintainer resolution, never agent repair,
  even when the same conversation also contains authorized findings. This gate precedes reply effects
  and repair handoff.
  `pullRequestFeedback: ignore` suppresses repair and responses but keeps unresolved-conversation gates.
  Recognized merge conversation rejections derive a provider-private reason from complete raw forge
  responses before diagnostic formatting or credential redaction; executable paths and printable
  diagnostics never authorize repair. They require fresh exact-head blocker evidence before returning
  to this gate; other API/authentication/policy failures keep their existing handling. Merge withdrawal
  remains mandatory before repair. Feedback is untrusted text and credentials remain delivery-owned.
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
  roots are run-owner `0700`, runtime roots supervisor-owned `0711`, and production ledgers and UI
  profile stores `0600`.
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
- Hosted suspend and vector POST routes use the separate `hosted_run_controls` discovery extension
  (`openengine.hosted-run-controls/v1`), relative to `hosted_runs.base_url`. Its strict
  `route_templates` contains `suspend`, `vectors` and `vector`; the existing hosted run/recovery
  route objects and the three capability markers stay unchanged.
- `export-workspace` downloads only from named hosted targets advertising the separate
  `openengine.workspace-export/v1` extension. Its one strict `route_template` is relative to
  `hosted_runs.base_url`; existing hosted run/recovery route objects stay unchanged. The HTTP-only
  `RunExportWorkspaceParams` contains run identity and an optional entry checkpoint, never storage
  authority. Shared target authentication/organization selection and same-origin, no-redirect rules
  apply. Export rejects dot-only run path segments before credential access. The CLI requires an
  explicit absent destination representable in its JSON receipt, stages with `platform::private_file`,
  consumes complete framed HTTP tar bytes, and uses `persist_noclobber` for atomic publication.
  Writes, flush and sync use Tokio file I/O so cancellation can be observed while disk work waits;
  the private path guard stays in the cancellable future, and flush errors prevent publication.
  HTTP/1 Transfer-Encoding overrides Content-Length; only one case-insensitive chunked coding is
  supported, otherwise a length is required with no Transfer-Encoding.
  Cancellation/body/status failures remove the stage; no local exporter, resume, or extraction exists.
- `template show` renders the materialized graph as a tree only on terminal stdout; piped or
  redirected stdout and explicit `--json` retain the complete, single-line `GraphSpec` JSON.
  The CLI graph renderer preserves authored structure and marks every step/verifier RuntimePlan
  binding; it does not infer or rewrite execution flow. Terminal detection belongs to the process
  entrypoint; generic output writers default to JSON.
- The foreground run command carries one Ctrl-C signal across preparation, submission, and
  observation. An interrupt before the backend submission future is first polled cancels without
  entering the backend. Once polled, that future is preserved through its receipt or error because
  it may cross an irreversible admission boundary before yielding. A successful receipt is emitted
  before detaching without opening observation. Ctrl-C never force-stops a run.
- Foreground `run` and `watch` settle a followed run the same way. A watch that closes without an
  ended snapshot settles from run status, which Ctrl-C detaches like observation. A suspended run
  ends either command with exit 0. A failed run
  ends either command with a process error (exit 1) that carries its latest bounded, control-safe
  error-log records and the `zeroshot logs` command. That read is best effort, deadline- and
  Ctrl-C-bounded, and never replaces the failure. `logs` exits 0 whatever the run's result.
  JSON diagnostics use `run_failed` / `run.failed` with `details.runId` for that terminal outcome.
  Unknown target names and registry workspace-recovery authorization refusals use
  `invalid_request` / `request.invalid`; transport failures retain `target.unavailable`.
  The Python SDK consumes the explicit failed-run diagnostic without inferring failure from a
  target error or rereading status; public failed-result exceptions remain opt-in. Failed-run
  identity stays private and unredacted for exact watch matching; its printable message is redacted.
  ACP turns reuse the bounded failure formatter when recorded terminal context exists; without
  context they retain `Zeroshot run failed: REASON`. Their `rawOutput` remains `{"failed":"REASON"}`.
- After opening observation, `run`, `watch`, `logs` and `attach` on direct or hosted named targets
  announce the first connection loss once on stderr and state that the run continues there.
  Reconnects back off from 100 ms to a five-second cap, preserving durable watch/log cursors.
  Ten continuous minutes without reconnecting ends the command with exit 1, the transport error
  and `zeroshot watch RUN_ID --target NAME`. Reconnect requests share that deadline. Ctrl-C
  detaches during requests and waits; neither detachment nor giving up stops the target run.
  Local controller observation keeps its existing lifetime; no reconnect setting is exposed.
  Named-target OECP observation opens only after the subscription acknowledgement, and dropping
  setup aborts its task. Python accepts a complete JSON error or the final stderr line after
  notices, preserving typed diagnostics; an earlier diagnostic never overrides the terminal line.
- Graph verification errors display their first safe diagnostic, its path and the count of further
  diagnostics through the shared verifier error, so local validation and hosted rejection report the
  same cause. Within the 1,024-byte target problem message bound, the count and then the path's
  innermost tail use only the bytes the message leaves.
- Do not hand-edit `docs/zeroshot-cli.md` or `docs/zeroshot-cli.html`; regenerate with
  `cargo run -p zeroshot --example generate_cli_docs -- --write` and verify with `--check`.
- Page descriptions live in Markdown front matter; generated reference descriptions live in
  `scripts/docs_hook.py`. `docs/overrides/main.html` renders social metadata and breadcrumbs.
  Search titles and social titles share the same template. Sitemap `lastmod` uses the verified
  content-change date from Git and is omitted when history cannot establish it.
  Fonts are self-hosted in `docs/assets/fonts/` with their licenses.
  The docs template loads the shared consent-gated analytics entry from zeroshot.sh;
  it only activates when the docs are served on that origin.
  The docs home draws zeroshot.sh's example graph inline from `docs/overrides/partials/example-graph-*.svg`;
  regenerate those with `docs/brand/freeze-example-graph.py`, never by hand. `docs/javascripts/command-wrap.js`
  keeps wrapped commands breaking only at spaces; it does not change copied text.
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
- A named-target run submission whose request may have reached the target but returned no receipt
  (timeout, interrupted connection or lost response body; not a refused or unestablished
  connection) is replayed identically with the same submission key, a bounded number of times inside
  the preserved submission future. Targets return the admitted run for an exact replay. A
  submission that stays unconfirmed reports `submission.outcome_unknown` with the key and
  `zeroshot list --target NAME`, never the unreachable-server hint.
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
- Hosted stacks (`zeroshot stack`, manifest `zeroshot.stack/v1`, discovery `zeroshot.stacks/v1`)
  are atomic, immutable DAGs over one explicit repository, base branch, and profile, delivered by
  merge or as stacked pull requests per the required manifest `delivery`. Merge stacks need one
  `merge@2`/`merge@3` profile delivery node and keep fan-in. Stacked pull-request stacks need one
  `pr@2` node and run as one linear stack: the CLI orders runs by dependencies, ties broken by name,
  and each run needs only its predecessor, starts from that run's branch and opens its PR against
  it. A run's PR head is `native_v2_delivery::delivery_head_branch` of its delivery run ID; receipts
  stay unchanged. Zeroshot never retargets or restacks PRs. The target resolves each node's exact
  source branch and revision only after its dependencies succeed; stacks have static inputs, no
  cross-node dataflow, no local execution, and no retry-in-place, and cannot contain wait nodes. Agent runtime bindings cannot
  declare `GH_TOKEN`; the sole delivery binding owns the GitHub write credential. A node's queue
  deadline is the earlier of stack expiry and seven days after readiness.
- Read-only safe commands include `zeroshot list`, `zeroshot status`, `zeroshot logs`,
  `zeroshot vectors`, and `zeroshot vectors show`.
- Destructive commands such as `zeroshot force-stop` require explicit user intent.

## Where to look

| Concept                       | Path                                                                                                                                                                                    |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Canonical crate and CLI       | `zeroshot/`                                                                                                                                                                             |
| CLI grammar/help              | `zeroshot/src/native_v2_cli/parser.rs`                                                                                                                                                  |
| CLI composition               | `zeroshot/src/native_v2_cli.rs`, `zeroshot/src/main.rs`                                                                                                                                 |
| CLI self-update               | `zeroshot/src/native_v2_cli/update.rs`                                                                                                                                                  |
| Built-in templates            | `zeroshot/src/native_v2_templates.rs`, `zeroshot/src/native_v2_templates/`                                                                                                              |
| Local run composition         | `zeroshot/src/native_v2_local.rs`                                                                                                                                                       |
| Hosted/platform composition   | `zeroshot/src/native_v2_platform.rs`, `zeroshot/src/native_v2_hosting.rs`                                                                                                               |
| Hosted stacks                 | `crates/openengine-cluster-protocol/src/native_v2_hosted/stack.rs`, `zeroshot/src/native_v2_cli/execution/stacks.rs`, `zeroshot/src/native_v2_target/controller_authority/hosted_runs/` |
| Portable controller           | `zeroshot/src/native_v2_portable_controller.rs`, `zeroshot/src/native_v2_portable_controller/`                                                                                          |
| Provider/delivery composition | `zeroshot/src/native_v2_candidate.rs`, `zeroshot/src/native_v2_candidate/`                                                                                                              |
| Target server                 | `zeroshot/src/native_v2_target.rs`, `zeroshot/src/native_v2_target/`                                                                                                                    |
| Target authority/auth         | `zeroshot/src/native_v2_target_authority.rs`, `zeroshot/src/native_v2_target_authority/`                                                                                                |
| Contained execution           | `zeroshot/src/execution.rs`, `zeroshot/src/execution/`                                                                                                                                  |
| Faults and redaction          | `zeroshot/src/fault.rs`, `zeroshot/src/fault/`                                                                                                                                          |
| Run ledger                    | `zeroshot/src/v2_run_ledger.rs`, `zeroshot/src/v2_run_ledger/`                                                                                                                          |
| Cluster protocol types        | `crates/openengine-cluster-protocol/`                                                                                                                                                   |
| Cluster server                | `crates/openengine-cluster-server/`                                                                                                                                                     |
| Cluster client                | `crates/openengine-cluster-client/`                                                                                                                                                     |
| Conformance fixtures          | `crates/openengine-cluster-testkit/`                                                                                                                                                    |
| Worker descriptors/registry   | `crates/openengine-cluster-protocol/src/worker.rs`, `crates/openengine-cluster-server/src/worker_registry.rs`                                                                           |
| Generated protocol artifacts  | `protocol/openengine-cluster/v1/`                                                                                                                                                       |
| Documentation site            | `mkdocs.yml`, `docs/`, `scripts/docs_hook.py`, `scripts/docs_versions.py`, `.github/workflows/docs.yml`                                                                                 |
| npm package                   | `npm/zeroshot/`                                                                                                                                                                         |
| Target image                  | `docker/zeroshot-target/`                                                                                                                                                               |
| Target declarations           | `distribution/zeroshot-targets.json`                                                                                                                                                    |
| Distribution tooling          | `scripts/distribution.js`, `scripts/distribution/`, `npm/zeroshot/lib/release-artifacts.js`                                                                                             |
| Python SDK                    | `sdks/python/`                                                                                                                                                                          |
| Release workflow              | `.github/workflows/release.yml`                                                                                                                                                         |
| Python release workflow       | `.github/workflows/release-python.yml`                                                                                                                                                  |
| CI classifier                 | `.github/ci-path-classifier.js`                                                                                                                                                         |
| Repository tooling tests      | `tests/tooling/`                                                                                                                                                                        |

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
  invented user stop or automatic retry. A suspended run has ended and is not interrupted.
- `WorkspaceServices` separates profile storage, native authoring, run listing, and observation.
  `RunHistorySource` supplies the viewer's run identity and transport. The standalone shell owns
  selection/navigation; graph components must remain independent of the host.
- Profiles use `LocalRunProfileStore` and `NativeV2Admission::validate_profile`. Saves check content
  revision and the bootstrap workspace UUID under the CLI's store lock. Identity persists with the
  store; replacement rejects stale tabs, including creation requests without a prior revision.
  Local storage retains complete profile entries as raw JSON. Listing borrows graph/runtime slices
  to inspect metadata and returns malformed entries separately in `problems`, without inventing IDs
  or hiding names. Only selected payloads become typed graphs/runtimes; selecting a malformed named
  or default profile fails explicitly rather than falling through to another scope. Deletion, default
  selection and unrelated saves preserve incompatible entries. Listing diagnostics belong to the local
  CLI/UI result, never the target's `zeroshot.run-profiles/v1` wire result. Legacy backend methods retain
  explicit failure when they cannot represent partial listings; the CLI diagnostic method defaults to
  wrapping unchanged target results. New entries are decoded through the selected-profile reader before
  publication so accepted writes remain readable; unchanged entries stay opaque. Shared store envelope
  corruption still fails closed. Checked saves decode the selected profile for its revision.
- Drafts/layout are scoped to workspace identity. Saves acknowledge only their captured document
  generation; imports carry no foreign revision. Pending numeric text belongs to the document,
  survives inspector navigation, and commits only when valid. Navigation must preserve unsaved work.
  Hosted workspaces use the `platform` kind with their existing opaque workspace IDs. The tag
  rename starts a new browser storage namespace; earlier drafts and preferences are not imported.
  Hosted environment saves use the same correlated snapshot/acknowledgement lifecycle: the iframe
  exposes only `{id?, name, definition, expectedRevision}`, while the host owns persistence and may
  atomically add private metadata. Host metadata participates in navigation protection through one
  opaque dirty boolean; its fields and domain semantics must not enter this repository.
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
  Wait input bindings preserve the fixed optional input schema when connecting, changing, or
  removing a source; a connected source must keep its declared input type.
  Removing a producer leaves typed, unbound inputs. Previous-attempt reuse requires native proof
  of the original source and compatible path. Label only unchanged caller values as Run input.
- Structural actions are atomic and preserve identities, bindings, and completion checkpoints.
  Parallel grouping checks dependencies; body replacement removes obsolete bindings. Converting a
  Verifier rejects remaining verifier-only contracts. Review loops carry feedback into the next
  attempt, starting with an implicit empty string; reviewers never inherit credentials or sessions.
- Artifact-producing work uses writing Agents; Verifiers perform independent checks/classification.
  Parallel/Map writers remain writers. Examples pass file paths and compact decisions, with separate
  files per writer and fresh directories per mapped item. Authors coordinate overlapping edits.
- Workspace bootstrap may include `machines` descriptors with size, label, vcpus, memoryMiB,
  diskGiB, laneUnits and optional GPU name/memoryMiB. The shared picker displays the box label and
  lane count, and derives the Boost count from laneUnits; hardware specs stay out of the run UI.
  The `runSizes` catalog supplies target-owned identifiers, descriptions and its default; machine
  facts only annotate offered choices. Without catalogs the editor accepts an opaque identifier,
  without inventing global choices. An explicit empty catalog offers none. Omitted/null sizes use
  the target default; unavailable authored sizes stay visible and unchanged. Hosts enforce
  availability and billing; descriptors are not profile data.
- The native template catalog exposes each complete authored default runtime, a concise description,
  and contained connection requirements derived through the harness registry. Code review preserves
  its supplied mixed Claude/Codex agent plan and native source-neutral delivery; catalog defaults omit
  target-owned size and enable checkpoints.
  `workspace::profile_setup` materializes hosted provider access after caller edits without saving,
  resolving secrets or replacing custom declarations. `setupLanes` exports the same registry's accepted
  provider field sets. Host-selected setup validates and materializes through the same registry as
  bootstrap/admission; local contributions remain unavailable to default and target setup.
  New graph/runtime definitions need no template-specific host wizard registry.
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
  A reducer `completed` group visit alone is neutral. The seq, choice, loop and map visits that a
  succeed/fail node ended at its cursor take its outcome; a par stops that propagation.
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
- Visual tokens/fonts live in `ui/src/styles.css` and `ui/src/theme.ts`; bundle all runtime assets.
  `embed.html` mounts the same editor/viewer without standalone menus. Its version-1 bridge
  validates the parent window and exact origin, binds workspace authority, and correlates document
  generations and save snapshots. Host service URLs remain same-origin; optional CSRF configuration
  carries cookie/header names only. The embedding host owns menus, authentication, conditional
  profile saves and live/archive adapters.
- Shared fetch-based SSE preserves HTTP and stream problem codes, accepted execution cursors,
  bounded frame buffering and owned reader cancellation. A host source change must preserve the
  viewer's history prefix and playback position; it never changes native graph projection.

## Issue triage

- The issue triage workflow adds `triage` to new and reopened issues. Only Maintain/Admin users
  and the verified support service App may assign or remove other issue labels; ordinary Write
  and Triage access does not authorize routing decisions.
- `scripts/github-issue-triage.js` derives current label intent from complete issue event history
  and fresh repository role checks. Per-issue workflow serialization may coalesce pending events,
  so reconciliation covers all labels instead of reversing only one webhook. Trigger visibility
  permits GitHub's observed one-second event/issue timestamp skew, with exact action/label/actor
  matching and bounded history rereads. The latest mutation for the triggering label must agree
  with fresh issue labels before role replay and again before writes; replay checks each actor.
- The policy comes from the trusted default branch. The workflow never executes issue text and
  uses only repository content read and issue write permissions.
- The guard's own `github-actions[bot]` corrections are not new label authority. Preserve later
  maintainer/service decisions across stale events and never replace the whole label list.
- Support-service backlog decisions remain authoritative for scheduling; GitHub labels alone
  never authorize a code change or merge. Bootstrap `triage`, `auto-fix-0` through `auto-fix-3`,
  and `plan-0` through `plan-3` before activating this workflow.

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
- Python test doubles use `openengine_cluster_testkit::fixture::python3_script`, which resolves
  the test PATH's actual interpreter before entering a pinned or cleared child environment.
- `.opcore.json` owns the full-source Opcore policy. PR quick checks run checksum-pinned Opcore Fast
  before queue admission. The reusable `.github/workflows/opcore.yml` checks the actual merge candidate
  with Fast and Cargo with warnings denied in merge-group/manual full CI. Both Fast passes skip
  root-README-only changes identified by the shared CI classifier. Cargo uses the complete checkout
  because Opcore's Rust snapshot omits embedded Python assets. The two exact PowerShell exclusions
  reflect unsupported parsing; the Windows native CI lane still runs those scripts.
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

- Build-time GitHub Release assets use `scripts/distribution/github-release-asset.js` and the
  authenticated API when `GH_TOKEN` is supplied. Credentials stay on `api.github.com`, never signed
  storage redirects. Callers retain their upstream checksum and immutable digest checks. CI supplies
  read-only tokens only to download steps; Docker uses the `github_token` BuildKit secret, never an
  image argument or environment layer. `build-restic.sh OUTPUT_ROOT --source-only` stages verified
  source for Go-only builders, which consume `ZEROSHOT_RESTIC_SOURCE_ARCHIVE` and recheck its pin.
- CI has native, Python, repository-tooling, npm-package, and strict-documentation lanes plus stable
  aggregate `required`. Pull requests run formatting, lightweight tooling and CI contract checks,
  then wait for authenticated exact-head Greptile completion. Full applicable lanes run only for
  merge groups or manual dispatch; ordinary main pushes do not repeat pre-merge validation.
  Reusable Opcore and native coverage participate in the full-CI aggregate. The aggregate rejects
  missing, failed and unexpectedly skipped checks for the selected event and paths.
  Root-README-only changes use a lightweight README lane for formatting, local
  links, and documentation contracts, without Rust or distribution checks. Mixed changes keep every
  applicable lane. `.github/select-ci-lanes.sh` shares history selection between CI and Opcore;
  empty, unavailable, or manually dispatched history selects full checks. The README lane participates
  in `required`, and missing or failed results block delivery.
  On PRs, that aggregate waits for Greptile's latest review check on the exact
  head to complete; findings remain review input for the worker rather than automatic CI failures.
  Merge groups use the completed PR gate because Greptile does not review merge-group commits.
  Native changes also require macOS product and test compilation with the UI
  feature, forge network/access and certificate trust regressions, workspace export integration and private publication regressions, focused Unix controller
  socket regression tests, and APFS workspace clone tests that
  `ZEROSHOT_REQUIRE_APFS_CLONE` keeps from skipping. `.github/ci-path-classifier.js` owns
  fail-closed path routing and cross-lane
  producer/consumer dependencies. The native lane runs on Linux and Windows, including real local
  CLI subprocess tests. Windows uses
  `scripts/test-windows.ps1` to run test executables outside Cargo's restrictive Job; the CI-only
  `.github/scripts/test-windows-host.ps1` also starts outside the hosted runner's Job. Linux also executes
  hosted process and filesystem boundary tests as root against its built test binary.
- `.github/workflows/coverage.yml` measures the default workspace, UI-feature Rust tests, and hosted
  root boundary tests on native changes, enforces coverage floors, and publishes LCOV to Coveralls.
  External publication is best effort and runs only with an actual LCOV report; measurement, tests
  and the local coverage floors remain required.
  It is reusable from the required merge-group CI aggregate and may also run manually.
  Linux coverage maps LLVM counters continuously with runtime counter relocation so post-fork
  helpers ending in `_exit` retain coverage without a profile-runtime call in the child. Profile
  filenames keep both process (`%p`) and binary (`%m`) identities to avoid unrelated writers.
  Tests limiting a child's file size direct only that child's `LLVM_PROFILE_FILE` to `/dev/null`
  so the intentional write failure cannot corrupt collected profiles.
  Local CLI fixture repositories exclude `*.profraw` through their shared Git exclude file,
  keeping profiles from environment-cleared instrumented processes out of delivery and status.
  Keep Rust test implementations in `tests/`, `tests.rs`,
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
  before documentation. Readiness checks retry only that returned ID with bounded polling before
  watching; they never redispatch. PyPI trusted publishing must remain in this standalone workflow, never in
  a reusable workflow. The same entry point handles later SDK-only revisions and exact recovery.
- `.github/workflows/docs.yml` publishes `main` as Current and canonical releases into `vX.Y/`
  after Python revision `1`; later SDK-only releases do not rebuild product documentation.
  The root always selects Current. Legacy `dev` and `stable` redirects stay outside the selector.
  Publication verifies the live manifest matches the validated docs, product and source identity.
- There is no automatic semantic release, release-promotion branch, `dev -> main` flow, or second
  runtime release train.
