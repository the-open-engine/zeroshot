# Run a first task

This example runs the built-in `software-change` graph in the current Git worktree, with pull-request
and merge delivery disabled.

!!! warning "The worker edits the current worktree"

    Commit or stash work you want to protect, and run the command from the repository you intend to
    change.

## 1. Write the input

The template expects an object with a `task` string:

```json title="input.json"
{
  "task": "Add JSON output to the status command and cover it with focused tests."
}
```

## 2. Choose the runtime

A uniform runtime applies one agent configuration to every agent node. Replace the model placeholder
with an identifier accepted by the selected provider.

```json title="runtime.json"
{
  "harness": "codex",
  "provider": "openai",
  "model": "YOUR_MODEL_ID",
  "effort": "high"
}
```

Zeroshot passes model identifiers to the provider unchanged and does not keep its own model catalog.
If `OPENAI_API_KEY` is absent from the environment, add the `openai` connection:

```console
zeroshot connection set openai --field OPENAI_API_KEY
```

## 3. Check the request without starting it

`--validate-only` materializes the template and uniform runtime, then admits the request with its
initial input. It does not contact a target or start a run.

```console
zeroshot run \
  --title "Add JSON status output" \
  --template software-change \
  --input input.json \
  --uniform-runtime-config runtime.json \
  --validate-only
```

Successful validation writes `{"valid":true}`. A rejected request names the input, graph, or runtime
problem; the selected target checks connection availability later, when it receives the submission.

## 4. Start the run

Remove `--validate-only`:

```console
zeroshot run \
  --title "Add JSON status output" \
  --template software-change \
  --input input.json \
  --uniform-runtime-config runtime.json
```

The foreground command streams newline-delimited JSON while the run advances. Ctrl-C closes that
stream without stopping the controller; another terminal can read the durable state:

```console
zeroshot list
zeroshot status RUN_ID
zeroshot logs RUN_ID
```

Normal completion happens when the graph reaches a terminal node. An explicit
`zeroshot force-stop RUN_ID` also closes the run, while engine or target failures can produce a
terminal failure result.

## Try the smaller template

`single-worker` has one agent node and no independent review loop:

```console
zeroshot template show single-worker
```

`zeroshot template show software-change` prints the exact graph behind the first example as the same
`GraphSpec` accepted by `--graph` and the Python SDK.
