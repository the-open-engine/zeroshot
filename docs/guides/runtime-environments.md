# Prepare a runtime environment

Create an environment once and select it from each profile that needs it. The shared Zeroshot
editor has a separate **Environments** view; a profile stores only its stable resource ID:

```json
{ "environment": { "id": "01990000-0000-7000-8000-000000000001" } }
```

A user profile references an environment owned by that user. An organization profile references
an environment in that organization. There is no fallback between scopes. A new run resolves the
latest saved definition and stores the complete result before acceptance. Editing an environment
affects future runs, including runs from other profiles that reference it. Queued and resumed runs
keep their accepted definition. Renaming an environment preserves its ID; delete it only after
removing its profile references.

`zeroshot acp` captures its local profile and environment when the ACP server starts. Restart that
server to use resource edits. ACP accepts public variables, while setup/startup hooks require a
Docker target and fail validation before an ACP session starts.

Direct Docker submission APIs still accept an exact environment definition in the runtime plan.
They do not need an environment catalog. A local CLI profile resolves its environment from the
local authoring store before sending the same concrete request to the target:


```json
{
  "environment": {
    "setup": "apt-get update && apt-get install -y jq",
    "startup": "npm ci",
    "variables": { "CI": "true" },
    "connections": { "package-registry": ["NPM_TOKEN"] }
  }
}
```

Store this definition in the environment resource when using profiles. Keep connection values
in the existing target connection store; scripts and public variables must not contain secrets. Only explicitly declared hook connections reach
preparation. Node connections remain independently declared.

Submission returns after admission and durable acceptance. While the run is `admitted`, setup runs
as root before source checkout or workspace restoration. Startup then runs as the workspace user,
with the checkout as its current directory. The graph starts only after both succeed. Watch run
logs for phase progress and command output. Nonzero exits become `environment_setup_failed` or
`environment_startup_failed`; a preparation deadline becomes `environment_preparation_timeout`.
These stop the run without using any graph repair attempts. Force-stop works during preparation
and waits for process cleanup before recording its terminal result.

Each hook uses Bash with `errexit` and `pipefail` and has fifteen minutes, inside one thirty-minute
preparation budget. Install OS packages in setup. Install shared executable tools under
`$ZEROSHOT_TOOLS/bin`, which is on the PATH of every hook and agent, and project dependencies in
the workspace. Shell exports affect only that hook; use `variables` for later commands. The
platform's harness Node interpreter remains separate from a project-installed Node version. Setup,
startup, and agent sessions have separate home directories; use `$ZEROSHOT_TOOLS` for tools shared
across them.

Workers and reviewers share files, ignored dependencies, generated output, and services. Startup
can start a background service; its process belongs to the run and survives individual agent
executions. Wait for required services to be ready before startup exits; the runtime does not infer
their health. Setup is for installation: start services from startup. Do not daemonize processes or
start root services in setup. Setup cancellation cleans up its shell process group; a root script
can escape that group, so this is not containment for untrusted root code. The target images
suppress Debian package service autostart. Parallel graph branches share mutable state, so scripts
and graphs must coordinate concurrent writes.

Both hooks run again on resume: setup, restore workspace files, startup, then restart or continue
the graph. Startup must be idempotent with respect to existing workspace files. Checkpoints do not
restore running services, Docker images or volumes, database transactions, or agent conversations.
A background process may change files while a checkpoint is captured; use application-managed
exports for consistent database recovery.

A direct Docker target is an operator-trusted container. Root setup changes that container's OS,
including for other runs it hosts. Use separate disposable target containers for independent OS
requirements. The target must have a writable root for package installation. Docker commands need
an operator-provided daemon endpoint. Detached Docker containers belong to that daemon and are not
terminated by the target's process cleanup. Direct target operators or scripts must manage their
names, reuse, and removal. When the daemon runs inside a disposable run machine, destroying that
machine removes its Docker resources. Local execution uses the invoking machine and rejects setup
and startup hooks.

## Local authoring API

`zeroshot ui` serves the resource API alongside profiles. Read `GET /ui/api/bootstrap` and include
its `workspace.id` as the `X-Zeroshot-Workspace` header on every write. Requests use JSON and the
same origin as the UI. These authoring operations never run hooks.

| Request | Result |
| --- | --- |
| `GET /ui/api/environments` | `{ "environments": [{ "id", "name", "revision" }] }` |
| `GET /ui/api/environments/{id}` | Resource with `id`, `name`, `definition`, and `revision` |
| `POST /ui/api/environments` | Creates or updates a resource |
| `DELETE /ui/api/environments/{id}` | Deletes an unreferenced resource |

Create a resource with this body:

```json
{
  "name": "node-project",
  "definition": { "setup": "apt-get update && apt-get install -y jq", "startup": "npm ci" },
  "expectedRevision": null
}
```

To update it, include its `id` and replace `expectedRevision` with the last returned revision.
Deletion requires `{ "expectedRevision": "<current revision>" }`. A stale revision or a duplicate
name returns `409 environment_conflict`; a referenced resource returns `409 environment_in_use`.
Missing resources return `404 environment_not_found`. Resource IDs remain stable across edits,
while revisions change after each save. Profile runtime JSON accepts the reference form shown
above; inline definitions belong only in concrete run submissions.
