# Active Memory

Active Memory preserves concise lessons about costly mistakes and non-obvious work likely to
recur. Local runs use the local Zeroshot state directory. A direct Docker target owns one store
shared by its submitters; repository scopes keep unrelated projects separate.

Workers do not write lessons. After the main graph settles, the built-in postmortem runs inside
the same run, using an existing node's model, provider and connections. It has four specialists:

- Graph maintenance reconciles every supplied graph lesson against its authored node instructions,
  output contract and procedural operation, including graph lessons qualified by a repository.
- Repository maintenance reconciles every supplied repository lesson against current authoritative
  files. Neither maintenance specialist receives or can modify lessons from the other scope.
- Graph discovery proposes procedural guidance for agent nodes that executed.
- Repository discovery proposes lessons about recurring work in the repository.

Each maintenance specialist keeps, adapts, retires or defers every lesson in its scope. Large
manifests are processed in complete batches bounded by serialized context size; every batch
requires exactly one decision per supplied ID. The host binds that ID to its frozen supplied
revision and owns revision increments; agents do not manage version counters. Missing delivery evidence makes the
specialists abstain. Missing or duplicate decisions fail that specialist's reconciliation instead
of reporting success.

Maintenance also consolidates overlapping advice within the same repository or exact graph/node/
repository-qualified scope. It keeps or adapts one canonical card and retires redundant paraphrases,
while still deciding every supplied ID/revision. Distinct causes and operations remain separate.

There is no findings count quota. Discovery returns every distinct lesson that qualifies; zero new
lessons is valid. A lesson needs evidence of expensive failure or substantial exploration,
a concrete recommendation and a reason to expect reuse. Task summaries, generic advice and facts
obvious from nearby code do not qualify. Titles have at most 96 UTF-8 bytes; recommendations have
at most 640. The store keeps evidence pointers and provenance separately from the injected card.
Use stable paths and identifiers in advice. Admission detects common literal root-relative `file:line` pointers; semantic maintenance
also checks other positional forms. Legacy cards remain readable; maintenance must adapt
them to stable guidance or defer, rather than keep or merely renumber their source positions.

Discovery checks observed main-run cost before accepting a proposal. Two timeout/crash completions
or two provably rejected review-loop visits can qualify. Repository discovery also accepts
exploration with at least three distinct tool callbacks spanning five minutes. A single ordinary
response-format correction does not qualify. Graph discovery requires the repeated-failure path.
Review cost is counted by distinct rejected rounds within one map scope, attributed to the workers
in that round's serial prefix. Supported layouts use a worker prefix followed by direct verifiers
or all-join verifier groups; error-only choice wrappers require a proven branch. The scanner checks
supported inner loops separately and skips enclosing nested loops or maps, incomplete/ambiguous
rounds, seeded histories and unsupported guards. Error exits are false only when every recorded
visit of that node has a settled non-error outcome; absent or unknown outcomes are not proof.
Repository discoveries also need a current file citation and a matching file, folder or verified
identifier association. These conservative checks establish cost;
the specialist must still establish
what was learned, why it recurs and whether current source supports it. Maintenance can update
supplied lessons even when discovery has no eligible evidence. Rejected discovery proposals log the failed
validation stage (including card field errors) without logging the proposed lesson text. Exploration
proof must come from the cited hooks themselves, rather than uncited tools elsewhere in the run.
Postmortem history is read in bounded pages; its fixed transcript budget reclaims old records
from the execution consuming the most space, so a verbose late reviewer cannot displace all
worker evidence. Reads stop between bounded pages on cancellation. Final context trimming shrinks
the largest evidence array first and retains an excerpt per execution where the byte budget
allows. Truncation remains explicit.

## Configuration

These optional sections belong in the runtime JSON, alongside `harness`, `provider`, `size` and
`nodes`. Both memory scopes and the postmortem default to enabled:

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

Set `repository` or `graph` to `false` to stop both retrieval and collection for that scope.
Set both to `false` to disable Active Memory. Set `postmortem.enabled` to `false` to retain recall
without collecting or maintaining lessons. These settings apply to each run, so saved profiles
can keep different policies.

`sourceNode` must identify an agent binding in the authored graph. Omit it to inherit the first
worker in graph order, then the first agent verifier. All four specialists inherit that node's exact
model, effective harness/provider lane, effort and declared connections. They use separate sessions. Each scope's opt-out disables both its maintenance and discovery.
A maintenance specialist cannot change the other scope even if its model returns a foreign ID.
Only built-in global agents are accepted initially.

`budgetBytes` is a cumulative context budget per provider session, including card framing. It must
be 1 through 16384; cards that do not fit remain undelivered. The postmortem's shared wall-clock
budget is `timeoutMs`, from 1 through 600000. Provider startup, investigation and all maintenance batches consume that budget. An interrupted or
failed maintenance pass never claims that the entire supplied manifest was reconciled.

## Indexing and retrieval

A run freezes an immutable, scoped snapshot. Matching uses resident inverted indexes and performs
no model calls, network lookups or filesystem scans. Repository lessons can independently associate
with multiple literal files, folders, identifiers or groups of keywords.

| Match                                 | Score |
| ------------------------------------- | ----: |
| Exact root-relative file              |  1000 |
| Containing folder                     |   800 |
| Exact whole identifier                |   700 |
| All terms in an indexed keyword group |   500 |

Each card gets its strongest independent match. Repeated words do not increase its score. Retrieval
sorts by descending score, then lesson ID, and returns complete cards within the remaining budget.
Impact determines admission, so retrieval ranks by relevance alone. An ID/revision is supplied once
per provider session. A new session can receive it again.

An identifier is a literal name such as `configure_bedrock` or `CLAUDE_CODE_USE_BEDROCK`. Matching
preserves the whole name and also splits snake/camel names into topic words for keyword groups.
Groups require two through four specific terms; individual broad words cannot retrieve a card.
Matching is deterministic and uses exact terms in this version.

Native path fields take priority. Paths resolve against the tool's working directory and the
workspace root, with component-aware folder matching. Shell arguments provide additional signals;
simple `cd folder && ...` commands are recognized. An opaque script that never exposes its file
paths can miss relevant cards. The matcher does not trace arbitrary program filesystem access.

Repository cards arrive through native harness hooks:

- Codex and Claude use `PreToolUse` additional context through a small native callback helper.
- Copilot uses its private RPC `preToolUse` callback.
- Pi uses a product-owned extension that appends cards to the tool result, including in contained
  mode where automatic project extensions remain disabled.

Context guides subsequent model decisions. It cannot revise a tool call already issued, so reads
and searches provide the most useful early triggers. Managed policy or disabled native hooks can
prevent injection. Transport failures leave the task running; missing delivery evidence makes
maintenance abstain. Hook receipts record returned context, not proof that the model followed it.

Graph lessons enter the node's instructions after admission validation. Their scope is the authored
compiled graph hash plus node ID, optionally repository. Memory injection never changes that hash,
the authored graph, permissions or output contracts. Authored graph changes create a new scope;
this version does not transfer guidance across graph versions.

## Persistence and run completion

Local runs share one database across repositories, outside repository checkouts:

| Platform      | Default database path                                         |
| ------------- | ------------------------------------------------------------- |
| Linux / macOS | `~/.local/state/zeroshot/active-memory/lessons.sqlite3`       |
| Windows       | `%LOCALAPPDATA%\zeroshot\state\active-memory\lessons.sqlite3` |

On Unix, `XDG_STATE_HOME` selects `$XDG_STATE_HOME/zeroshot/active-memory/lessons.sqlite3` when set.
`ZEROSHOT_STATE_DIR` takes precedence on every platform and selects
`$ZEROSHOT_STATE_DIR/active-memory/lessons.sqlite3`. Changing these variables selects another store;
it does not move existing lessons.

Executable and skill updates preserve this directory, as does normal run and workspace cleanup.
A direct target uses `<target-storage>/active-memory/lessons.sqlite3`; preserve its storage volume
when recreating the Docker container.

Zeroshot does not create automatic backups. To back up or move memory, stop all Zeroshot processes
using the store and copy the whole `active-memory/` directory, including any SQLite journal files.
Restore it while those processes are stopped. Deleting the state directory or losing the storage
volume loses its lessons unless you have a backup.

The host owns the database outside the checkout and publishes all shared-store mutations. Workers
receive a run-scoped retrieval capability. Reconciliation uses expected revisions, atomic transactions and
deletion tombstones, so concurrent runs cannot silently overwrite a newer lesson or recreate a
retired record from a stale snapshot. Newly published lessons become available to future runs.

Maintenance can replace lessons or retire obsolete ones. Previous contents are not archived, and
there is no built-in undo; deletion tombstones retain IDs and revisions rather than card contents.

The main graph outcome is durably latched before postmortem. Ordinary postmortem failures appear in
run logs and preserve that outcome. After a controller restart, confirmed runtime cleanup preserves
the latched outcome and marks interrupted auxiliary token usage incomplete. Cancellation stops
and drains the optional work; uncertain provider
cleanup retains the runtime's existing failure handling. Global-agent work stays outside reducer
execution history and checkpoint seeds, while its model usage contributes to run totals.

The native hook records bounded tool receipts in memory for postmortem evidence, including when a
tool matches no card. Receipts contain the node, tool name, time and a few associated workspace
paths; they exclude arguments and results and do not claim that a tool succeeded. The first and
latest receipts survive a fixed 512-record cap. Hidden reasoning is not required or inspected.

Local and target stores do not synchronize automatically. Cloud integration and ACP integration
are outside this version. Embedders must explicitly select a host-owned memory namespace; the
generic hosting configuration does not create an organization-wide store implicitly.

## Initialize from local history

Use the dedicated `zeroshot-memory-init` skill to initialize memory from all qualified local run
history. It orchestrates separate discovery subagents for each selected run, then fresh graph and
repository maintenance specialists against the recorded current checkouts.

`zeroshot update` installs or refreshes the skill; npm installation includes it too. If upgrading
through an older updater that only knows the `zeroshot` skill, run `zeroshot update` again after the
binary update. See [skill installation](../zeroshot-distribution.md#canonical-identity) for user-scope
locations and conflict handling.

The skill uses one resumable command, independent of the invoking folder:

```console
zeroshot memory init --directory /path/to/session
zeroshot memory init --directory /path/to/session --ignore-repos owner/repo,other-repo --ignore-graphs autoresearch,GRAPH_HASH
```

`--ignore-repos` and `--ignore-graphs` accept comma-separated, space-separated or repeated values.
Matching is exact and case-sensitive. Repository values match aliases reported in the inventory:
checkout folder name, absolute checkout path, admitted repository name, forge `owner/repo`, or the
opaque `repo:...` namespace. A short folder name excludes every matching repository. Graph values
match the exact authored root name or graph hash reported in the inventory; original profile names
are not retained in historical admission. There is no `--cwd` flag.

The first call creates a private session outside repository checkouts and inventories finished local
native runs across all repositories. Each run carries its own repository namespace, aliases,
workspace, graph name and hash. Checkouts are resolved from recorded workspace metadata, with
current remote identity checked against admitted sources. Admissions without a remote are matched
against repository IDs independently resolved from the current checkout or its registered remote;
folder names and historical display aliases cannot prove identity. A historical remote, when present,
still requires its exact remote-derived identity. During inventory, unavailable or changed checkouts are
explicit exclusions and do not prevent other repositories from proceeding. Nothing is cloned or
fetched. Current checkout probes are shared across runs.

Subsequent calls return the next work through `phase` and `next_action`. The skill classifies every
candidate, fills the exported specialist response files, and calls the same command to advance.
Saved ignore lists apply on resume without repeating flags; conflicting lists require a new session.
Discovery findings remain staged until every supplied card has a maintenance decision. The CLI
validates the same impact rules and lesson contracts as after-run postmortems; it makes no model
calls and uses no provider credentials. Host subagents use the host's model connections.

The newest selected run for each repository supplies its recorded current checkout for maintenance.
Graph maintenance covers each exact graph and repository scope; unqualified graph cards shared by
several repositories are reconciled once. Repository cards stay within their own repository.
Specialist contexts never include another repository's cards. Every selected checkout is fingerprinted,
and all mutations across repositories publish in one transaction after complete maintenance coverage.
Remote and runtime namespace identities are rechecked before review and publication, including
changes to Git configuration that leave tracked files unchanged. If a selected checkout disappears or
changes identity after staging, publication stops; restore it or start a new session with appropriate
exclusions. Supplied cards are never silently dropped. Citations through symlinks require their
canonical targets to be tracked or nonignored too, so the cited bytes participate in the fingerprint.

`phase: complete` returns the publication receipt. Retrying a completed session returns that result
without repeating mutations. Session locks prevent concurrent advancement, including interrupted
history export. Atomic SQLite receipts recover a process interruption between publication and its
local completion checkpoint; checkpoints use durable atomic file replacement before receipts are
removed. Source or lesson changes preserve staged findings for fresh maintenance work; digest-bound
task responses prevent stale reuse. Deferred discoveries remain inactive. There is no findings count
quota.

Current source includes tracked and nonignored untracked files and executable permissions. Ignored
artifacts cannot be maintenance authority. Keep code stable during final review and publication;
the filesystem and database cannot share a transaction. The CLI does not pull or change branches.
Historical exploration receipts were not persisted, so backfill cannot reconstruct them from
elapsed time. Unreadable, unfinished, opted-out and unprovable-repository runs remain explicit
exclusions. Old ledgers are read-only; initialization usage belongs to the host session.
Cloud and Docker target history are outside this workflow.

Inventory can prove an attached run's repository from a unique eligible remote even when its
recorded checkout now has detached HEAD. Publication uses that checkout's current runtime namespace,
reported per run. Existing local path namespaces do not migrate automatically when a checkout later
acquires an admitted remote source.
