---
description: "Run the same graph locally, on a self-hosted Docker target or on a managed target. Compare source preparation, connections and durable run state."
---

# Targets

A target owns source preparation and the processes that execute graph nodes; it also stores durable
run state and resolves connections. The graph protocol stays the same when the target changes.

## Local controller

Omit `--target` to work in an existing local folder. `--cwd FOLDER` selects the folder for one run;
the default is the invoking folder. Input, graph, runtime, and environment file arguments remain
relative to the invoking folder. A folder inside a Git checkout resolves to its canonical checkout
root. Resume uses the original stored workspace.

A run without Git delivery also accepts plain folders, remoteless repositories, detached checkouts,
and unborn repositories. Zeroshot starts a detached controller and keeps the ledger under the local
state directory, where later CLI invocations can list, watch, or stop the run. It does not initialize
Git to run in a plain folder.

The workspace and active Zeroshot state directory must be disjoint, including symlink aliases.
If either contains the other, select another folder or set `ZEROSHOT_STATE_DIR` outside the workspace.
Existing inactive state folders do not impose a separate admission rule. Disjoint workspaces can
run concurrently. Aliases share one lease; a folder and its subtree cannot run at the same time. Reusing a submission key for
another folder returns a conflict rather than a run from the first folder.

Local execution is the only mode that mutates the caller's existing folder; each selected
installed harness runs as the current user. A plan with
[per-node lanes](../reference/runtime-plan.md#per-node-lanes) needs every lane's CLI installed. The
native `codex`/`openai`, `claude`/`anthropic`, and `copilot`/`github` lanes also need that CLI
signed in; other lanes need their provider's
[connection values](../reference/runtime-plan.md#connections). Before the controller starts,
`zeroshot run` and `zeroshot resume` check that each lane's executable is on `PATH`; they do not
check login state, so a missing login fails at that lane's first turn.

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

Local Pi runs use the current user's `~/.pi/agent` for a `pi/anthropic` or `pi/openai` lane, which
reuses a stored Pi login. Any other Pi lane, a declared credential, or a caller-owned endpoint
instead receives a private agent directory so no ambient catalog or credential is inherited. A
declared or ambient `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, or `OPENAI_API_BASE` becomes a Pi
provider override that keeps that provider's own protocol, and `pi/gateway` carries
`GATEWAY_BASE_URL`, `GATEWAY_API_KEY`, and an authored `GATEWAY_API` wire protocol. Pi exposes no
permission control, so Pi runs add no bypass flag; containment remains the boundary.

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

Local and hosted verifiers operate directly on the candidate: the restriction on edits is instruction
guidance, not a filesystem boundary. Hosted nodes share the prepared workspace and its owner identity.
They retain separate session homes and process cleanup markers, so ending one node does not terminate
another node or the run's startup services.

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

### Workspace snapshot disk usage

Workspace snapshots recursively copy entries without applying `.gitignore`, including generated
directories such as `target/` and `node_modules/`.

Zeroshot tries filesystem cloning or reflinks where supported. Otherwise, each capture needs space
for another full workspace copy during staging, in addition to Restic checkpoint storage. Concurrent
runs increase peak disk usage and can fail with `no space left on device`.

On tight disks, clean unnecessary build artifacts or keep large generated directories outside the
workspace where practical.

## Direct target

A direct target exposes Zeroshot's HTTP and OECP contracts without application-level authentication.
The released container includes Zeroshot plus pinned Codex, Claude, GitHub Copilot, and Pi harnesses.
A run can mix their lanes; its connection requirements are then the union over its nodes, such as
`OPENAI_API_KEY` and `ANTHROPIC_API_KEY` for a Codex worker with Claude Code reviewers.

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

Use `--target local-target` on run and observation commands to select it. Docker and hosted targets
require a Git source with an exact committed revision. `--cwd` selects the client checkout inspected
for source selection; targets receive the remote revision, not that local folder or its uncommitted
files. The attached upstream takes precedence; otherwise select the unique eligible HTTPS or SSH
remote. Ambiguous remotes require an upstream or an explicit `--repository` selection.

`--repository` accepts a credential-free HTTPS/SSH clone URL or the existing GitHub `owner/name`
shorthand. `--branch` overrides the branch, and `--revision` supplies an exact commit instead of
resolving the remote branch tip. Detached worktrees require repository and branch overrides.
Generic transport and credentials require a target advertising `openengine.git-transport/v1`;
the CLI rejects an older target before submitting the request or acquiring an access token.

### Image tags and remote hosts

The image is published for `linux/amd64` only. Each release publishes a version tag such as
`ghcr.io/the-open-engine/zeroshot-target:X.Y.Z` and a `sha-COMMIT` tag. `latest` moves to each newer
release, so pin a version tag or an image digest when a host must keep the same image.

To use a target on another machine, keep the published port on that host's loopback interface, as in
the command above, and open an SSH tunnel from your workstation:

```console
ssh -N -L 8080:127.0.0.1:8080 USER@HOST
zeroshot target add remote-target --url http://127.0.0.1:8080 --direct
```

Keep the tunnel open while you use the target. If a local target already uses port 8080, forward
another local port and start the remote target with `--public-origin` set to that local origin,
such as `http://127.0.0.1:18080`; the target advertises its endpoint from that origin. The
[target image README](https://github.com/the-open-engine/zeroshot/blob/main/docker/zeroshot-target/README.md)
covers reverse proxies, Docker access for runs, and building the image.

## Hosted target

A hosted target adds discovery and user authentication around the same native contracts:

```console
zeroshot target add production --url https://TARGET_ORIGIN
zeroshot target login production
```

The hosting service owns login, source authorization, organization-scoped connections, queue policy,
and capacity. Zeroshot's CLI and protocol do not prescribe those product policies.

Follow the [Platform documentation](https://app.zeroshot.sh/docs) for Zeroshot Platform. Its pages
should link to the version of these core docs that matches the deployed target, as described in
[Documentation versions](../project/versioning.md).

### API keys

Hosted targets that advertise API key authentication support machine delegations. Sign in as a
human to create one:

```console
zeroshot target login platform
zeroshot delegation create ci
zeroshot delegation create release --owner org --organization ORGANIZATION_UUID
```

Any user can delegate their own current permissions. Organization owners and admins can create
organization delegations. Creation prints an `api_key` once; store it as `ZEROSHOT_API_KEY` in your
CI secret store. Keys expire after 90 days by default; `--expires-in-days` accepts 1–365.

With the key set, use the normal CLI with `--target platform`, including connections, profiles, merge
plans, live sessions, and resume. For a user key, set `ZEROSHOT_ORGANIZATION` to the organization
UUID when organization selection is needed. Organization keys stay in their own organization.
Omitted connection and profile scopes follow the owner's scope.

Delegation management requires human login. Unset `ZEROSHOT_API_KEY` before managing keys:

```console
zeroshot delegation list
zeroshot delegation key create DELEGATION_ID --name rotation
zeroshot delegation key revoke DELEGATION_ID OLD_KEY_ID
zeroshot delegation disable DELEGATION_ID
```

Create the replacement before revoking the old key. Revoking a key blocks further calls while
accepted runs continue. API keys authenticate the CLI; keep provider and source credentials in
their usual connections.

## Source and delivery credentials

Git checkout and push use shared HTTPS or SSH transport. A local push needs an attached, committed branch
and an eligible remote; it uses the invoking user's native Git configuration and authentication when
no source connection is selected. Push does not require GitHub or its CLI. PR and merge additionally
require a registered forge adapter; GitHub is the maintained forge today. Other Git hosts can already
use checkout and push.

For local PR and merge delivery, the target branch is the current branch's upstream when configured;
otherwise, Zeroshot uses the local branch name. Preparation verifies that the selected remote
contains the target before agent execution. A fresh worktree may not have an upstream configured. To
target `main`, run `git branch --set-upstream-to=origin/main` in the worktree, or create it from
`origin/main` so Git configures the tracking branch. A missing branch is reported separately from a
remote access or authentication failure.

`--source-connection KEY` selects private transport authority without granting it to agent nodes.
HTTPS connections declare `GIT_USERNAME` and `GIT_PASSWORD`; SSH connections declare
`GIT_SSH_PRIVATE_KEY` and `GIT_SSH_KNOWN_HOSTS`. Explicit SSH authentication uses those pinned hosts
and its private key. Contained runs inherit no host Git login state.

Named GitHub runs retain `GH_TOKEN` checkout and delivery support. That token is not automatically
applied to another Git host. A model provider receives a connection field only when its own runtime
binding declares it. Transport values stay out of source records, ledgers, observations, and
checkpoint snapshots. Direct-target resume reuses the original source connection authorization and
resolves fresh values.

After admission, the CLI reports the target, repository, branch, exact revision, and whether the
client checkout is dirty before emitting the receipt. Uncommitted files stay on the client.
See [Source-code adapters](../project/source-code-adapters.md) for the contribution boundary.
