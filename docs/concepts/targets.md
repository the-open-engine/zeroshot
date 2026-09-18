# Targets

A target owns source preparation and the processes that execute graph nodes; it also stores durable
run state and resolves connections. The graph protocol stays the same when the target changes.

## Local controller

Omit `--target` to work in the current Git worktree. Zeroshot starts a detached controller and keeps
the run ledger under the local state directory, where later CLI invocations can list, watch, or stop
the run.

Local execution is the only mode that mutates the caller's existing worktree; the installed Codex or
Claude harness runs as the current user.

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

Local and hosted workers default to Codex's `--dangerously-bypass-approvals-and-sandbox` or Claude's
`--dangerously-skip-permissions` when no permission policy is configured. Before each turn, Zeroshot
queries the harness's resolved settings without sending a model prompt. Explicit approval, sandbox,
permission rules, and managed restrictions take precedence: Zeroshot adds no bypass flag in those
cases. Claude shell permission controls are inherited locally too. If inspection fails or the CLI
does not support it, the harness keeps its native permission behavior. Inspection adds one CLI
startup per turn and is bounded to ten seconds; it does not rewrite settings files, though the CLI
may update its own startup state.

Local verifiers operate directly on the candidate and retain Codex's read-only sandbox or Claude's
plan permission mode. Hosted verifiers receive disposable writable copies and use the same
permission defaults as workers. The hosted process and filesystem isolation remains in force.

Zeroshot controls the response format and session continuation. Web search follows Codex's
configuration: by default it uses live search with full access and cached search otherwise.
An explicit `web_search="cached"` also prevents Zeroshot from adding bypass, because Codex can
otherwise promote it to live search under full access. Explicit web-search settings remain unchanged.
Command network access follows the selected sandbox policy, including any configured restrictions.

Declare any environment variables required by a custom provider's `env_key` or environment-based
headers in the runtime's connections. A declared `OPENAI_API_KEY` remains available under that name
for custom providers. Zeroshot also supplies `CODEX_API_KEY` for native OpenAI authentication when
that variable isn't already declared. See [Runtimes and connections](runtimes-and-connections.md).

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
marks interrupted runs as lost.

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
