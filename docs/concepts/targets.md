---
description: 'Choose local execution, a self-hosted Docker target or zeroshot Platform. Connect a target and prepare its source and credentials.'
---

# Targets

A target is where your agents run and their run history is stored. Choose it independently of the
graph and models.

| Target        | Use it for                              | Source                                             |
| ------------- | --------------------------------------- | -------------------------------------------------- |
| Local         | Work in a folder on your machine        | Your existing files, including uncommitted changes |
| Direct Docker | Run on infrastructure you manage        | An exact committed Git revision                    |
| Hosted        | Use a service such as zeroshot Platform | An exact committed Git revision                    |

The model provider still receives the code and context its agent needs. Self-hosting the target
does not make model processing local.

## Run locally { #local-controller }

Omit `--target` for a local run. Add `--cwd FOLDER` to choose the workspace; otherwise zeroshot uses
the invoking folder. A folder inside a Git checkout resolves to its root. Input and configuration
file paths remain relative to where you invoked the command.

Local agents edit this workspace as your user. With `--delivery none`, Git is optional. Install and
sign in to every agent CLI your runtime plan uses. zeroshot checks for executables before starting;
a missing login fails at the agent's first turn.

Keep the workspace separate from the active zeroshot state directory. Independent workspaces can
run concurrently, but overlapping folders cannot. Set `ZEROSHOT_STATE_DIR` outside the workspace
if needed.

Local Codex and Claude runs respect configured permission policies. When none is configured,
zeroshot may add permission-bypass flags. Reviewers share the workspace; their instruction not to
edit reviewed files is not filesystem isolation. See
[local authentication and permissions](../reference/target-runtime.md#local-workspace-and-authentication)
for each harness's behavior.

### Workspace snapshot disk usage

Snapshots include ignored files such as `node_modules/` and `target/`. Where filesystem cloning is
unavailable, staging needs another full workspace copy as well as checkpoint storage. Concurrent
runs increase peak usage. Remove unnecessary build artifacts or keep large generated directories
outside the workspace when disk space is tight.

Restic failures report the operation and failure stage or exit status without exposing private
child output. A failed backup leaves the previous published checkpoint available.

## Run a Docker target { #direct-target }

The Docker image includes zeroshot and its core agent CLIs. It is published for `linux/amd64`.
Start a persistent target and register it with your CLI:

```console
docker run --detach --restart unless-stopped --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest

zeroshot target add local-target \
  --url http://127.0.0.1:8080 \
  --direct
```

!!! danger "Direct targets have no authentication"

    Keep the port on loopback or a trusted private network. Before broader exposure, add an
    authenticated reverse proxy that forwards WebSockets.

Add `--target local-target` to run and observation commands. Open `http://127.0.0.1:8080/ui/` for
profiles and run history. The `zeroshot-data` volume preserves both when the container is recreated.
If you change the browser origin, set `--public-origin` to match it.

Docker must be running. The restart policy recovers from reboots and unexpected exits; after a
manual stop, use `docker start zeroshot-target`. Interrupted runs are marked lost. If recovery data
exists, start a successor with `zeroshot resume RUN_ID --target local-target`.

Docker and hosted runs use the remote Git revision, not your uncommitted files. `--cwd` chooses the
client checkout used to select that source. Its upstream takes precedence; otherwise zeroshot
selects the unique eligible HTTPS or SSH remote. For ambiguous remotes, set an upstream or pass
`--repository` with a credential-free clone URL or GitHub `owner/name`. Use `--branch` and
`--revision` to select a branch and exact commit. Detached worktrees need repository and branch
overrides. Generic Git transport requires a target advertising `openengine.git-transport/v1`.

If a submission reply is lost, zeroshot retries with the same submission key. If it still cannot
confirm the result, check `zeroshot list --target NAME` or rerun with the reported
`--submission-key KEY` to recover the same run.

### Image tags and remote hosts

Pin `ghcr.io/the-open-engine/zeroshot-target:X.Y.Z` or an image digest for a stable deployment.
`latest` advances with releases; `sha-COMMIT` tags identify their source.

For a remote machine, keep its port on loopback and open an SSH tunnel:

```console
ssh -N -L 8080:127.0.0.1:8080 USER@HOST
zeroshot target add remote-target --url http://127.0.0.1:8080 --direct
```

Keep the tunnel open. If port 8080 is taken locally, forward another port and set the target's
`--public-origin` to that origin, such as `http://127.0.0.1:18080`.
The [target image README](https://github.com/the-open-engine/zeroshot/blob/main/docker/zeroshot-target/README.md)
covers proxies, Docker access and image builds.

## Use a hosted target { #hosted-target }

For zeroshot Platform, run `zeroshot target login platform`, then use `--target platform`.
To register another hosting service:

```console
zeroshot target add production --url https://TARGET_ORIGIN
zeroshot target login production
```

The service manages login, source access, organization connections, queues and capacity.
See the [Platform docs](https://app.zeroshot.sh/docs) for its setup.

### API keys

For CI, sign in as a human and create a delegation:

```console
zeroshot target login platform
zeroshot delegation create ci
```

Store the one-time `api_key` as `ZEROSHOT_API_KEY` in your CI secret store. It carries your current
permissions. Keys expire after 90 days by default; `--expires-in-days` accepts 1–365.
Set `ZEROSHOT_ORGANIZATION` to the organization UUID when a user key needs organization selection.

Organization owners and admins can create an organization delegation instead:

```console
zeroshot delegation create release --owner org --organization ORGANIZATION_UUID
```

Organization keys stay in their organization. Omitted profile and connection scopes follow the
owner's scope. Keep provider and source credentials in their usual connections.

Unset `ZEROSHOT_API_KEY` and use human login to manage delegations. Create a replacement key before
revoking the old one:

```console
zeroshot delegation list
zeroshot delegation key create DELEGATION_ID --name rotation
zeroshot delegation key revoke DELEGATION_ID OLD_KEY_ID
zeroshot delegation disable DELEGATION_ID
```

Revocation blocks further calls; already accepted runs continue.

### Workspace export protocol

To download files, see [Download a retained workspace](../guides/observe-and-control.md#download-a-retained-workspace).
For hosting integrations, see the [workspace export contract](../reference/target-runtime.md#workspace-export-protocol).

## Sizes and supported agents

Run `zeroshot target sizes NAME` to list available sizes; add `--json` for structured output.
Omitting a size uses the target's default. Local execution and direct targets offer `host`, using
the machine's resources. The target decides which other sizes are available.

Official local builds include contributed adapters; Platform and the default Docker image include
core adapters only. Check [harness support](../project/harness-adapters.md) before choosing a runtime.
[Target discovery](../reference/target-runtime.md#discovery-and-sizes) specifies size and adapter negotiation.

## Source and delivery credentials

Local Git operations can reuse your Git authentication. PR and merge delivery support GitHub and
GitLab; other Git hosts support checkout and push. See [forge setup](../guides/forge-delivery.md).

**Breaking change:** in-place local delivery commits on the checked-out branch and pushes the
remote branch of the same name, including existing unpushed commits. It creates no managed run
branch. PR/MR targets use a different upstream branch, otherwise the remote's default branch.
`--pr` and `--ship` refuse to start on that target; use a feature branch or `--worktree`.
Explicit `--push` can publish any checked-out branch, including `main`. Non-fast-forward pushes
and forge policy refusals stop delivery without asking agents to repair them.

`--worktree` selects a separate retained worktree on a managed run branch. It starts at local HEAD,
including committed unpushed work. `--worktree-base remote` fetches the invoking branch's upstream;
without one, configure it or choose `--worktree-base local`. Delivery commits and publishes that
same managed branch. PR/MR targets use the invoking branch's upstream, or its own name when none
is set. zeroshot verifies the target before starting agents.

The invoking checkout's files, index, HEAD and branch tips remain unchanged in worktree mode.
Only committed files enter the new worktree; ignored dependencies, `.env` and untracked resources
are neither copied nor linked, and zeroshot performs no dependency setup. Both successful and
failed worktrees remain for inspection. Resume uses the recorded workspace and keeps HEAD attached
to its run branch without rewinding shared refs. Remove it explicitly with
`git worktree remove PATH` after inspection. Named targets and hosted runs keep their managed run
branches and do not use these local flags.

Contained targets do not inherit your Git login. `--source-connection KEY` selects transport
credentials: `GIT_USERNAME` and `GIT_PASSWORD` for HTTPS, or `GIT_SSH_PRIVATE_KEY` and
`GIT_SSH_KNOWN_HOSTS` for SSH. These credentials are separate from agent model connections.

Named targets normally resolve GitHub access from their stored `github` connection. To forward
local `GH_TOKEN` for GitHub.com checkout and delivery, pass `--forward-gh-token` to `run`, `resume`
or `stack submit`. The flag fails if the token is unset and does not apply it to another Git host.
It can also fill a declared `GH_TOKEN` connection field and support direct-target `--task-from`
resolution. Model nodes receive credentials only through their declared connections.

Transport secrets stay out of source records, history and checkpoints. Direct-target resume keeps
the original source authorization and resolves fresh values. The submission receipt reports the
selected repository, branch and revision; uncommitted files stay on the client.
