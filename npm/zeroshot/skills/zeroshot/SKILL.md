---
name: zeroshot
description: Use Zeroshot to prepare, run, observe, or troubleshoot explicit multi-agent software work locally or on Zeroshot Platform. Apply when the user names Zeroshot, including 'zeroshot this' or 'zeroshot it' (this orchestrator, not one-shot prompting), or asks about its CLI; do not route ordinary local work into Zeroshot without that intent.
---

# Zeroshot

Zeroshot executes an explicit graph of software agents. Workers make changes; independent verifiers
judge the evidence, and the graph decides when to repair or deliver. A successful run means the
graph reached its declared outcome; it does not always mean a pull request was created or merged.

Use the installed CLI help as the exact contract when commands differ from this overview.

## Shape the task

Before submitting, inspect the request, repository guidance, existing issue, and enough code to know:

- the desired behavior and observable success;
- material constraints and behavior that must remain unchanged;
- whether a separate worker could implement it and a verifier could judge it.

Fill gaps from available evidence. Choose routine technical details without interrogating the user.
If an unresolved product choice would materially change the outcome, propose the narrowest supported
interpretation and ask one focused question. Do not submit while that decision remains unanswered.
Suggest separate tasks only for independently useful outcomes or real prerequisites.

When the work is vague or large enough to need agreed acceptance criteria, suggest planning it first
with the separate `zeroshot-plan` skill and let the user choose; do not start planning unasked. When
a plan file exists (`.zeroshot/plans/<id>.md`), use its Goal, Context, Acceptance criteria,
Constraints, Decided approach and Decisions as the task. Criteria marked `Check (manual):` need a
person or another environment: tell the run to list them rather than verify them. If the plan lists
`depends_on`, confirm with the user that those plans are built and merged before starting it, and
include any constraints they share that the plan does not repeat.

Never start a Zeroshot run merely because the skill is available. The user must ask to use Zeroshot.
A worker already executing inside a Zeroshot run should perform its assigned work directly, not start
another run. A failed Platform run does not authorize silently implementing the task locally.

## Choose the target

- Local execution omits `--target`. `--cwd DIR` selects a folder (default: invoking folder);
  Git worktrees use their root. File arguments stay relative to the invoking folder. Runs without
  Git delivery need no repository. Keep workspace and active Zeroshot state disjoint; use an external
  `ZEROSHOT_STATE_DIR` when necessary.
- Zeroshot Platform is built in. Sign in once with `zeroshot target login platform`, then use
  `--target platform` explicitly on Platform commands. Platform setup (organizations, GitHub App,
  connections) is documented at https://app.zeroshot.sh/docs.
- Do not add or set up the built-in Platform target. Named targets store endpoints and login identity,
  not repository configuration.

## Reuse profiles and connections

For a guided interview to design or revise a graph, use the `zeroshot-graph-design` skill.

Prefer a saved profile over inventing a graph or runtime. Inspect its input, runtime, and delivery
behavior before constructing a run. Local profiles need no login; a local default profile applies
when a run names no profile, graph, or runtime:

```console
zeroshot profile list
zeroshot profile show NAME
zeroshot profile list --target platform --scope user
zeroshot profile list --target platform --scope org
zeroshot profile show NAME --target platform --scope org
zeroshot connection list --target platform --scope user
zeroshot connection list --target platform --scope org
```

Inspect saved profiles to see their delivery behavior. New `software-change` templates default to
pull-request delivery. Use `--delivery none` for local-only changes, `--push` for branch publication,
or `--ship` when merge delivery is intended. Custom graphs and saved profiles own their delivery.

## Run locally without a profile

When no saved profile fits and the user has not asked for Platform, run the built-in
`software-change` graph locally. Its default publishes a PR. For local-only work, use
`--delivery none` as shown below; that mode needs only the harness's native login.

1. The worker edits the selected folder in place. Stop if it has uncommitted changes, and start
   from a fresh branch when the folder is a Git repository.
2. Write the shaped task to an input file outside the workspace, for example
   `{"task": "Add JSON output to the status command and cover it with focused tests."}`.
3. Write a uniform runtime file for the harness you are running in, with your own model identifier
   unless the user names another. Local Claude Code and Codex lanes reuse their native login:

   ```json
   { "harness": "claude", "provider": "anthropic", "model": "MODEL_ID", "effort": "high" }
   ```

   ```json
   { "harness": "codex", "provider": "openai", "model": "MODEL_ID", "effort": "high" }
   ```

4. Validate, then submit the same command without `--validate-only`. This example keeps the work
   local; omit `--delivery none` for PR delivery, or use `--ship` when the user asks to merge:

   ```console
   zeroshot run --title "TASK" --template software-change --delivery none \
     --input INPUT.json --uniform-runtime-config RUNTIME.json --validate-only
   ```

5. Follow the run with `zeroshot status RUN_ID` and `zeroshot logs RUN_ID`. The user can watch it
   in `zeroshot ui` at `http://127.0.0.1:4173/ui/`.
6. Report what each reviewer rejected and what each repair changed, not only the final state. If
   the run exposed a gap the built-in reviewers do not cover, name one reviewer worth adding.
7. Offer to save the setup as the default, so later requests need no files:

   ```console
   zeroshot profile set NAME --template software-change --delivery none \
     --uniform-runtime-config RUNTIME.json --default
   ```

## Review an issue or pull request

The `pr-review` and `issue-review` templates post reports as comments by default.
PR review classifies whether reproduction is necessary, runs required full-stack checks (including
a browser for UI changes), then checks the diff and linked issue coverage. Its final report is concise. PR review uses comment delivery; issue review also supports `--delivery none` for a report without publication. Supply an issue or PR URL through `--task-from`, which resolves
title/body and pins source before execution:

```console
zeroshot run --title "Review PR 123" --template pr-review \
  --task-from https://github.com/OWNER/REPOSITORY/pull/123 \
  --uniform-runtime-config RUNTIME.json --cwd /path/to/pr-checkout \
  --submission-key review-pr-123
```

Use `issue-review` for an issue URL; its basic issue workflow is separate. PR intake captures linked
issue text before execution. Local reviews require clean tracked files and, for a PR, a checkout already at its head. Comment
publication uses the declared `github.GH_TOKEN` connection. Use `--target platform` for hosted source
preparation and credential resolution. With a saved review profile, use `--profile` instead of the
template/runtime/delivery flags. Validate first with `--validate-only`; retry uncertain submission
using the same key and configuration. A successful review may report defects; success confirms
publication. Ordinary `--input FILE` passes the graph's JSON input unchanged and excludes
`--task-from`. Worker nodes have no mandatory `task` field.

## Adapt the graph

When the user wants to change what gets checked, start from the built-in graph rather than writing
one from scratch:

1. Export it with `zeroshot template show software-change > graph.json`.
2. Make the change the user asked for: add a reviewer with one job (security, an end-to-end test
   that boots the app, design), or change the loop's `maxIterations`. Different models or harnesses
   per node need a full `--runtime-config` plan instead of the uniform file. The
   [review loop guide](https://the-open-engine.github.io/zeroshot/current/guides/review-loop/)
   shows the node shapes and a cross-vendor plan.
3. Validate by replacing `--template software-change` with `--graph graph.json` in the run command
   above, then run it.
4. Save it so later requests use it, with `--runtime-config PLAN.json` in place of the uniform file
   when nodes use different models or harnesses:

   ```console
   zeroshot profile set NAME --graph graph.json \
     --uniform-runtime-config RUNTIME.json --default
   ```

5. When runs keep failing the same way, or a problem gets past the reviewers, suggest the reviewer
   that would catch it and ask before changing the graph.

## Prepare the execution environment

Environments belong to a run, independently of its profile. A `local:NAME` profile can run on a
Docker target with its own environment. For Docker or Platform, pass
`--environment environment.json` with a flat definition such as:

```json
{
  "setup": "apt-get update && apt-get install -y jq",
  "startup": "npm ci",
  "variables": { "CI": "true" }
}
```

Platform resolves an omitted environment from the repository's User default, then Org default, then
Base. `--no-environment` bypasses defaults. Choose saved environments through Platform's UI or API;
the CLI accepts a concrete JSON file. Local runs use the invoking machine and reject setup/startup
and hook connections.

Setup runs as root before checkout or restoration, so it cannot read repository files. Startup
runs as the workspace user in the checkout; workers and reviewers share its dependencies and
services. Both hooks rerun on resume, so startup must tolerate existing workspace files.
Checkpoints restore files and graph progress, not running services or agent conversations.

Put shared executables in `$ZEROSHOT_TOOLS/bin` and project dependencies in the workspace.

Snapshots include ignored directories such as `target/` and `node_modules/`. Without cloning or
reflinks, allow space for another full workspace copy during staging, plus checkpoint storage.
Before large or parallel runs on tight disks, warn users and suggest cleaning build artifacts or
keeping generated directories outside the workspace where practical.

Shell exports do not persist between hooks or agents; use `variables` for nonsecret values and
`connections` for required secret field names. Hook connections and node connections are separate.
On a direct Docker target, root setup changes the container shared by its runs.

Preparation failures stop before agent execution without spending repair attempts. Inspect the
failed phase and logs; fix the environment instead of requesting unrelated code repairs. Resume
reuses the accepted definition, so an edited environment requires a new run. For a pinned Node
installation and service/Docker conventions, see the
[environment guide](https://the-open-engine.github.io/zeroshot/current/guides/runtime-environments/).

## Expose a profile over ACP

Start `zeroshot acp` only when the user explicitly asks to expose a Zeroshot graph as an ACP agent.
The experimental endpoint is local-only and owns stdio until its client disconnects:

```console
zeroshot acp --profile local:NAME
```

Check the profile first. It must accept exactly one required string named `task`; every success node
must return exactly one required string named `response`. The runtime must use Codex or Claude for
every node, and every executable node must have an agent binding with `sessionScope: node_instance`.
Maps, delivery, runtime connections, MCP servers, and session reload are unsupported.

Each ACP prompt is a separate durable local run. The outer ACP session keeps the workspace and
node provider sessions alive across prompts. Inspect the returned run ID with `zeroshot status` or
`zeroshot logs`; `zeroshot ui` includes the run in local history.

## Validate and submit

Put profile input in a JSON file matching the profile's declared schema. Use a stable, non-secret
submission key for safe retries:

```console
zeroshot run --title "TASK" --profile org:NAME --input input.json \
  --target platform --submission-key KEY --validate-only

zeroshot run --title "TASK" --profile org:NAME --input input.json \
  --target platform --submission-key KEY
```

`--validate-only` materializes and validates without starting a run. A named Platform profile must
still be fetched from the target first, so this command requires Platform access. It does not prove
connection availability.

For named targets, `--cwd` selects the client Git checkout. Zeroshot prefers its attached upstream,
then a unique eligible HTTPS/SSH remote. Docker and Platform still require an exact committed Git
source. Detached worktrees require repository and branch overrides. `--repository` accepts GitHub
`owner/name` or a credential-free HTTPS/SSH clone URL. Override source only when needed:

```console
--target platform --repository OWNER/REPOSITORY --branch BRANCH --revision SHA
```

Git push uses shared transport and needs no GitHub API. PR and merge require a registered forge;
GitHub is maintained today. `--source-connection KEY` authorizes source-only HTTPS
(`GIT_USERNAME`, `GIT_PASSWORD`) or pinned SSH (`GIT_SSH_PRIVATE_KEY`, `GIT_SSH_KNOWN_HOSTS`)
credentials. Generic forms require the target's `openengine.git-transport/v1` capability.

Named targets receive the remote revision, not uncommitted files. Check dirty or unpushed work and
never push or omit it without authorization. Confirm the exact repository, branch, revision, target,
profile, and delivery intent reported for the run.

## Observe and troubleshoot

Keep the run ID and target:

```console
zeroshot status RUN_ID --target platform
zeroshot watch RUN_ID --target platform
zeroshot logs RUN_ID --target platform
```

A foreground run streams events. Ctrl-C detaches observation without stopping the run. Use
`zeroshot force-stop RUN_ID --target platform` only when the user explicitly intends to stop it.

For command errors, check `zeroshot --version` and the relevant `--help`; do not revive removed
commands such as `target setup`. If login cannot retain credentials, fix the credential store before
requesting another device code; `zeroshot target login --help` lists the supported
`ZEROSHOT_CREDENTIAL_STORE` modes.

For `connection_unavailable`, list connections for the same target and both relevant scopes. A
static `connection set` replaces the whole named connection, so supply every required field through
repeated `--field` prompts or JSON stdin. Never put secret values in arguments, task files, runtime
configuration, issues, or logs.

After an uncertain submission response, reconcile by submission key before retrying. Do not create a
second logical run. Report submitted, running, verified, delivered, failed, and stopped states
accurately; submission alone is not completion.

## Initialize Active Memory

For initialization or backfill from local run history, use the separate `zeroshot-memory-init`
skill. It orchestrates isolated discovery followed by graph and repository maintenance through the
resumable `zeroshot memory init` command.
