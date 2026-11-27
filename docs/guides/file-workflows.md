---
description: 'Author a file workflow, inspect a published draft, answer with a file and retrieve the final folder through the CLI, Python SDK or a capable host.'
---

# Work with files and folders

The [example graph](../assets/file-workflow/file-workflow.graph.json) reads a supplied source
folder, writes `report/draft.txt` and returns JSON `null`. A request publishes that folder for
inspection and asks for a `filepath` answer. A later worker reads the answer and edits the draft.
Graph success publishes the resulting folder through a typed state binding, independently of its
`null` result. Error branches stop before consuming missing outputs.

The `produce` and `edit` nodes need agent runtime bindings; `approval` needs `{ "kind": "request" }`.
For example, save this as `runtime.json`, replacing `PROVIDER_MODEL_ID` with a model available to
your configured provider. Omit `size` to use the selected target's default:

```json title="runtime.json"
{
  "harness": "codex",
  "provider": "openai",
  "workspaceCheckpoints": true,
  "nodes": {
    "produce": { "kind": "agent", "model": "PROVIDER_MODEL_ID" },
    "approval": { "kind": "request" },
    "edit": { "kind": "agent", "model": "PROVIDER_MODEL_ID" }
  }
}
```

## CLI path

Save the graph as `file-workflow.graph.json`. Put source files in a folder named `source`.
Upload it to a reusable library and separately stage a private filepath input:

```bash
zeroshot file upload ./source examples/source
zeroshot file stage ./source
```

Put the complete staged receipt in `input.json` as the `source` value:

```json
{
  "text": "Inspect the draft and supply an edit file.",
  "source": { "kind": "staged", "runId": "UPLOAD_ID", "operation": "UPLOAD_OPERATION" }
}
```

Start a managed, delivery-free workspace seeded from the library. The seed keeps its basename
`source` inside the working folder; the filepath input gets a separate run-owned path:

```bash
zeroshot run --title "Edit a published folder" --graph file-workflow.graph.json \
  --runtime-config runtime.json --input input.json --workspace \
  --workspace-seed user:examples/source
zeroshot inbox show RUN_ID
```

The run suspends at the request. The authorized inbox response's `request.artifacts.draft`
contains a managed reference. Use its `address.path` and producing `address.scope.runId` to
retrieve the draft:

```bash
zeroshot file download PUBLICATION_PATH ./draft-download --run PRODUCING_PART_ID
zeroshot file stage ./edits.txt
```

Put the new staged receipt in `answer.json`:

```json
{
  "signals": {},
  "output": {
    "reply": { "kind": "staged", "runId": "ANSWER_UPLOAD_ID", "operation": "ANSWER_OPERATION" }
  }
}
```

```bash
zeroshot answer RUN_ID --input answer.json
zeroshot watch RUN_ID
zeroshot status RUN_ID
zeroshot file list --run FINAL_PRODUCING_PART_ID
zeroshot file download FINAL_PUBLICATION_PATH ./final-download --run FINAL_PRODUCING_PART_ID
```

Final references are in `status.metadata.artifacts`. Follow each reference's producing part after
an answer or resume; the logical run ID can differ. Downloads require a new destination. Use
`--member draft.txt` to retrieve only that member of the published folder.

Omitting `--target` selects local state. Add the same `--target NAME` to **every** file, run, inbox,
answer, observation and cleanup command to use a registered direct or capable hosted target.
File discovery is independent of run support: missing or unknown `openengine.target-files/v1`
support refuses the operation. A target never falls back to your local filesystem.

## Python path

The [complete async example](https://github.com/the-open-engine/zeroshot/blob/main/sdks/python/examples/file_workflow.py)
performs upload, staging, submission, request inspection, filepath answer and final download:

```bash
python sdks/python/examples/file_workflow.py --graph file-workflow.graph.json \
  --runtime runtime.json --source ./source --answer ./edits.txt --output ./final-download
```

Add `--target NAME` for a registered direct or capable hosted target. `Client` also accepts an
explicit `DirectTarget`. The SDK delegates transfer and validation to its packaged CLI; file bytes
stream through the native transport. `stage_file`, `upload_file`, `list_files`, `download_file`,
`remove_file` and `cleanup` return the corresponding CLI JSON receipts. Existing typed errors and
task cancellation apply. `RunResult.artifacts` contains final references;
`(await run.request()).request.artifacts` contains request-authorized attachments. Ordinary status
and history readers receive no request attachment metadata.

`RunRequest(workspace=True, workspace_placement="managed", workspace_seeds=("user:examples/source",))`
selects a seeded workspace. These options also work with string submissions. For a local folder,
use `workspace=True, cwd="/path/to/folder"` to work in place, or `copy_workspace=True` to freeze
that folder into a disposable managed copy. In-place placement is local only; named targets use
managed workspaces. Seeds and copying select managed placement and cannot be combined.

## Current bytes and lifetime

Uploading, staging, admission of a library seed, and publication each capture their selected
current bytes. Later edits to a local original or a library name do not replace accepted inputs
or an earlier managed publication. The next publication of an edited draft captures its new
contents. File inputs and answers are always managed copies under `ZEROSHOT_INPUTS`, even when
the working folder is local and in place. They stay outside Git staging.

An in-place publication is a `kind: "local"` reference to its recorded root and relative path.
It denotes live current contents, can change or disappear, and creates no managed publication.
Open that path on the same machine. Managed references remain downloadable with the CLI/SDK after
the disposable workspace has gone. Ordinary string output paths do not publish anything.

Disk reclamation is explicit:

```bash
zeroshot cleanup
zeroshot cleanup --all
```

The default age is 60 days. `--all` removes that age filter for eligible managed publications and
discardable checkpoint lineages on the selected supporting target. Active, queued, suspended and
restoring work stays protected. Library files, live local outputs, logs and ledgers stay outside
cleanup. A cleaned publication cannot be downloaded; accepted input pins retain their own bytes.
See [target files](../reference/target-files.md) for retry identities, exact lifetime and transfer
rules.

## Shared editor and embedding hosts

The shared graph editor offers **File or folder** fields and literal workspace paths or typed
filepath bindings under request attachments and final published files. Environment seeds select
user or organization library paths and optional members. A host resolves saved environment seeds
into the submission's `workspace.seeds`; they never become profile runtime settings.

Hosts own run launch, authorized file selection, downloads and sandboxed preview delivery. The
shared inspector displays authorized metadata and offers inspection/download only when the host
advertises `openengine.workspace-files/v1` in its existing embedded `init.files`. Request
attachment reads require their separate advertised authorized reader; run-read permission alone
does not expose them. Older hosts and standalone viewers keep working, with CLI/SDK retrieval
hints for final managed publications and live-path labels for local results.

The [embedded host contract](https://github.com/the-open-engine/zeroshot/blob/main/ui/README.md#file-capabilities)
specifies the optional metadata route and `file_action` bridge message. Actions carry metadata,
never an inferred filesystem grant or a download URL. The host rechecks authority before opening
anything. Advertise only features the consuming host implements.
