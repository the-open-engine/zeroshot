# Targets

A target owns source preparation and the processes that execute graph nodes; it also stores durable
run state and resolves connections. The graph protocol stays the same when the target changes.

## Local controller

Omit `--target` to work in the current Git worktree. Zeroshot starts a detached controller and keeps
the run ledger under the local state directory, where later CLI invocations can list, watch, or stop
the run.

Local execution is the only mode that mutates the caller's existing worktree; the selected installed
harness runs as the current user.

Local Codex runs with `provider: "openai"` use the model provider and transport configured in the
user's Codex configuration, including OpenAI-compatible proxies such as LiteLLM. Zeroshot supplies
the runtime's model and any explicit reasoning effort, but leaves web search and sandbox network
access to Codex's configuration. Explicit `openrouter`, `bedrock`, and `gateway` selections configure those
providers; hosted targets use their own isolated provider setup.

Local Claude runs use the current user's home and `CLAUDE_CONFIG_DIR`, when set. Local runs
also inherit the shell's endpoint variables, including `OPENAI_BASE_URL`
for Codex and `ANTHROPIC_BASE_URL` for Claude. Declared connection values take precedence over these
shell defaults. Hosted adapters receive declared connection values without inheriting the host's
shell settings. Explicit gateway selections use their `GATEWAY_BASE_URL` connection value. Local
`anthropic` runs also inherit Claude's shell provider-selection flags. Explicit `openrouter`,
`bedrock`, and `gateway` selections retain their provider setup and reject declared transport flags
that would route to an incompatible provider.

Local Copilot runs use the current user's `COPILOT_HOME` and system credential store. Ambient
`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN` values are selected in native precedence and
sent through private RPC rather than the Copilot process or its tools. Copilot endpoint, proxy,
certificate, and custom-provider environment settings are inherited locally. Static BYOK endpoint,
credential, header, wire, and model-mapping settings are materialized into the private Copilot RPC
session and removed from agent tool environments. `COPILOT_PROVIDER_API_KEY_COMMAND` crosses the
singular private provider contract, which lets the pinned CLI refresh rotating keys for each request.
Eligible helper-only invoking-shell fields are all marked through Copilot's `--secret-env-vars`, so
they remain available to the internal provider command but are not inherited by agent shell or MCP
environments. Runtime, auth, provider, and parent-process loader controls are excluded.
That filtering is not a separate OS identity boundary against adversarial same-UID processes.
A bounded `COPILOT_PROVIDERS_CONFIG` or default `COPILOT_HOME/providers.json` registry crosses as
protocol provider/model values because the headless server does not import that path into sessions;
a nonempty registry wins over legacy provider variables. An explicit declared token suppresses
ambient BYOK and offline controls. Declared endpoint and credential fields cannot accidentally retain
ambient credentials for a different route. The runtime model remains the process-level registry
selection while provider `modelId` capability and wire-model mappings stay intact. A registry
command translated to the singular RPC contract uses its authored `modelId` as that session's model.
Contained targets use a private Copilot home and require the canonical `COPILOT_GITHUB_TOKEN`
connection instead.

The local target intentionally has the same-user trust boundary as running the harness CLI directly:
native config, user files, and credential-store access remain subject to that harness's own sandbox
and tool policy. Zeroshot transfers the invoking shell through a private, one-shot bootstrap and
deletes it before the controller performs run effects; the detached controller itself receives only
minimal platform environment. A later local CLI startup removes a bootstrap orphaned beyond the
controller handoff window. This snapshot is never written to a run ledger or sent to a hosted target.

Hosted Codex and Claude workers and verifiers always run with
`--dangerously-bypass-approvals-and-sandbox` or `--dangerously-skip-permissions`, respectively.
Zeroshot does not inspect the harness's native permission policy for hosted turns; the disposable
capsule is the isolation boundary.

Local Codex and Claude workers and verifiers use those flags only when no permission policy is
configured. Before each local turn, Zeroshot queries the harness's resolved settings without sending
a model prompt. Explicit approval, sandbox, permission rules, and managed restrictions take
precedence, so Zeroshot adds no bypass flag in those cases. Claude shell permission controls are
inherited locally too. If inspection fails or the CLI does not support it, the harness keeps its
native permission behavior. Inspection adds one CLI startup per local turn and is bounded to ten
seconds; it does not rewrite settings files, though the CLI may update its own startup state.

Every agent verifier node, including those in custom profiles, uses the same permission handling as
workers. Zeroshot automatically adds instructions to inspect without modifying source, tests,
configuration, or other material under review or implementing repairs. Verifiers may run checks and
create temporary files and generated artifacts, and must report failed or unavailable checks accurately.
The built-in acceptance verifier also tries to run the repository's pre-commit checks and reports
the results.

Local verifiers operate directly on the candidate: the restriction on edits is instruction guidance,
not a filesystem boundary. Hosted verifiers retain disposable writable copies; their process and
filesystem isolation remains in force.

Zeroshot controls the response format and session continuation. Web search follows Codex's
configuration: by default it uses live search with full access and cached search otherwise. For local
runs, an explicit `web_search="cached"` prevents Zeroshot from adding bypass because Codex can
otherwise promote cached search to live search under full access. Hosted turns always use maximum
bypass inside the disposable capsule, so Codex may promote cached search to live. Command network
access follows the selected sandbox policy locally; hosted command isolation comes from the capsule.

For local Codex, Zeroshot reads the active provider's `env_key` and environment-based header names
from Codex's resolved configuration, then forwards only those values from the invoking shell to the
model process. This lets custom OpenAI-compatible gateways keep using their existing Codex setup.
Declared connection values take precedence, and a declared `OPENAI_API_KEY` remains available under
that name. Zeroshot also supplies `CODEX_API_KEY` for native OpenAI authentication when needed.
Values discovered from local configuration are not persisted or sent to hosted targets. See
[Runtimes and connections](runtimes-and-connections.md).

## Direct target

A direct target exposes Zeroshot's HTTP and OECP contracts without application-level authentication.
The released container includes Zeroshot plus pinned Codex, Claude, and GitHub Copilot harnesses.

```console
docker run --detach --restart unless-stopped --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest

zeroshot target add local-target \
  --url http://127.0.0.1:8080 \
  --direct
```

The target keeps running between runs. Docker restarts it after a reboot or unexpected exit with
`--restart unless-stopped`; manually stopping it keeps it stopped until `docker start zeroshot-target`.
Docker itself must be running. Stopping the target stops its runtime; restart retains history and
marks interrupted runs as lost. When recovery data is available, start a successor with
`zeroshot resume RUN_ID --target local-target`.

The image's `target serve` process also serves the UI at `http://127.0.0.1:8080/ui/`.
Profiles and run history persist under `--storage` in the named `zeroshot-data` volume, including
when you recreate a container previously started with `--rm`. If you change the published port or
use a proxy, set `--public-origin` to the exact browser origin.

!!! danger "Direct means unauthenticated"

    Keep the published port on loopback or a trusted private network. Put an authenticated reverse
    proxy with WebSocket forwarding in front of it before broader exposure.

Use `--target local-target` on run and observation commands to select it. Each named run selects
its repository and branch from the invoking worktree's attached upstream. Without an upstream, the
worktree must have exactly one GitHub remote. `--repository` and `--branch` override that selection,
and `--revision` selects an exact commit instead of resolving the current remote branch tip.
Detached worktrees require explicit repository and branch values.

## Hosted target

A hosted target adds discovery and user authentication around the same native contracts:

```console
zeroshot target add production --url https://TARGET_ORIGIN
zeroshot target login production
```

The hosting service owns login, source authorization, organization-scoped connections, queue policy,
and capacity. Zeroshot's CLI and protocol do not prescribe those product policies.

Follow the [Cloud documentation](https://dev.theopenengine.com/docs) for Zeroshot Cloud. Its pages
should link to the version of these core docs that matches the deployed target, as described in
[Documentation versions](../project/versioning.md).

## Source and delivery credentials

For named-target runs, the CLI can send `GH_TOKEN` for source checkout and generated GitHub delivery.
A provider receives that value only if its runtime binding declares `GH_TOKEN`. After the target
accepts a submission, the CLI reports the target, exact `owner/repository@branch#revision`, and
whether the invoking worktree is dirty before emitting the run receipt. Uncommitted files remain
local and are never committed or pushed automatically.
