---
description: 'Reuse lessons from earlier runs. Configure Active Memory, preserve its storage and initialize it from local run history.'
---

# Active Memory

Active Memory carries useful lessons from one run to the next. It saves guidance about costly
mistakes and recurring work, then supplies relevant lessons when an agent encounters related code
or runs the same graph again. It is enabled by default.

After the main graph finishes, a postmortem reviews the evidence, updates existing lessons and
proposes new ones. Workers do not write directly to memory. Routine summaries and generic advice
do not qualify as lessons.

## Configuration

Add these optional fields to your runtime JSON alongside `harness`, `provider`, `size` and `nodes`:

```json
{
  "activeMemory": {
    "repository": true,
    "graph": true,
    "budgetBytes": 2048
  },
  "globalAgents": {
    "postmortem": {
      "enabled": true,
      "sourceNode": "worker",
      "timeoutMs": 120000
    }
  }
}
```

- Set `repository` or `graph` to `false` to stop reading and collecting lessons for that scope.
  Set both to `false` to disable Active Memory.
- Set `postmortem.enabled` to `false` to keep reading saved lessons without updating them.
- `budgetBytes` limits memory context per provider session, including formatting; the range is
  1–16384 bytes.
- `sourceNode` selects the agent whose model, provider and connections the postmortem uses.
  Omit it to use the first worker, or the first agent verifier if there is no worker.
- `timeoutMs` limits the postmortem to 1–600000 milliseconds, including startup and investigation.

These settings apply per run, so saved profiles can use different policies. Postmortem model usage
counts toward the run total. An ordinary postmortem failure appears in the logs and does not change
the main graph's result.

## Indexing and retrieval

Repository lessons match file paths, folders, identifiers and keyword groups. Graph lessons belong
to a particular compiled graph and node; changing the graph creates a new scope. Lessons do not
transfer automatically across graph versions.

Each run reads a snapshot of memory. New lessons become available to future runs. Matching makes
no model calls. A lesson is supplied once per provider session, within the context budget.

Repository lessons arrive through harness hooks. Disabled hooks can prevent delivery; scripts that
hide their file paths can miss matches. Advice cannot change a tool call already issued, and receipt
of a lesson does not prove the model followed it. See the
[matching and delivery rules](../reference/active-memory.md#indexing-and-retrieval) for details.

## Storage and backups { #persistence-and-run-completion }

Local runs share a database outside your checkouts, with separate repository scopes:

| Platform      | Default database                                              |
| ------------- | ------------------------------------------------------------- |
| Linux / macOS | `~/.local/state/zeroshot/active-memory/lessons.sqlite3`       |
| Windows       | `%LOCALAPPDATA%\zeroshot\state\active-memory\lessons.sqlite3` |

On Unix, `XDG_STATE_HOME` changes the base directory. `ZEROSHOT_STATE_DIR` takes precedence on all
platforms and selects `STATE_DIR/active-memory/lessons.sqlite3`. Changing either variable selects a
store; it does not move existing lessons.

A direct Docker target uses `<target-storage>/active-memory/lessons.sqlite3`, shared by its
submitters with repository scopes. Preserve its storage volume when recreating the container.
Local and target stores do not synchronize automatically. ACP integration is not supported.

Executable updates and normal run cleanup preserve memory. There are no automatic backups or
built-in undo for changed or retired lessons. To back up or move it, stop all zeroshot processes
using the store and copy the whole `active-memory/` directory, including SQLite journal files.
Restore it while those processes are stopped.

## Initialize from local history

Ask your agent to use the `zeroshot-memory-init` skill to learn from completed local runs.
`zeroshot update` installs the skill; if it is missing after upgrading through an older updater,
run the update again.

The skill inventories local history across repositories, discovers lessons, checks them against
current source and publishes them together. Keep the selected checkouts stable during its final
review and publication. It does not clone repositories, pull changes or change branches.
Docker and Platform history are outside this workflow.

The skill uses this resumable command:

```console
zeroshot memory init --directory /path/to/session
zeroshot memory init --directory /path/to/session --ignore-repos owner/repo,other-repo --ignore-graphs autoresearch,GRAPH_HASH
```

Choose a session directory outside checkouts. Ignore values must match inventory aliases exactly;
saved exclusions apply when resuming. Unavailable checkouts and unusable runs are reported as
exclusions. `phase: complete` returns the publication receipt; repeating a completed session does
not publish twice.

See [history initialization](../reference/active-memory.md#history-initialization) for alias matching,
repository identity checks and interruption recovery.

## Concurrent maintenance and hosting

Concurrent runs check lesson revisions before publishing, so stale updates cannot overwrite newer
advice. Hosting integrations must explicitly provide a memory namespace and enforce its access.
See [memory lifecycle and storage](../reference/active-memory.md) for maintenance, evidence and host
contracts.
