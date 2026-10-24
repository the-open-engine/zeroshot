# Run a first task

This example runs the built-in `software-change` graph in a local folder, with Git delivery disabled.
The template normally creates a PR; this example explicitly uses `--delivery none`.
Git is optional. Add `--cwd FOLDER` to select another folder; input files stay relative to the invoking folder.

!!! warning "The worker edits the selected folder"

    Back up work you want to protect, or commit or stash it in a Git checkout.

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
For this local run, Zeroshot reuses the installed Codex CLI's login and provider configuration; no
placeholder API key or `openai` connection is required. A Docker or cloud run instead derives the
canonical `OPENAI_API_KEY` requirement and resolves it from the submission environment or the
target's connection store because it cannot inherit host login state.

For another harness, use its file instead. A local run reuses that CLI's own login in the same way.

```json title="runtime.json for Claude Code"
{
  "harness": "claude",
  "provider": "anthropic",
  "model": "YOUR_MODEL_ID",
  "effort": "high"
}
```

```json title="runtime.json for GitHub Copilot"
{
  "harness": "copilot",
  "provider": "github",
  "model": "YOUR_MODEL_ID"
}
```

```json title="runtime.json for Pi"
{
  "harness": "pi",
  "provider": "anthropic",
  "model": "YOUR_MODEL_ID",
  "effort": "high"
}
```

The [RuntimePlan reference](../reference/runtime-plan.md#harness-and-provider) lists every accepted
pair, including the OpenRouter, Bedrock, and gateway lanes that need a connection.

## 3. Check the request without starting it

`--validate-only` materializes the template and uniform runtime, then admits the request with its
initial input. It does not contact a target or start a run.

```console
zeroshot run \
  --title "Add JSON status output" \
  --template software-change --delivery none \
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
  --template software-change --delivery none \
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

For a graph view with node transcripts, open [`zeroshot ui`](install.md#open-the-workspace-ui)
and select the run in **Runs**.

Normal completion happens when the graph reaches a terminal node. An explicit
`zeroshot force-stop RUN_ID` also closes the run, while engine or target failures can produce a
terminal failure result. `zeroshot logs RUN_ID` keeps the error records that explain a failure,
such as a failed node's error or the runtime cause. A failed foreground run exits with status 1 and
its error lists the latest of those records, followed by the `zeroshot logs` command for the rest.

## Try the smaller template

`single-worker` has one agent node and no independent review loop:

```console
zeroshot template show single-worker
```

`zeroshot template show software-change --delivery none` prints the exact graph behind the first example as the same
`GraphSpec` accepted by `--graph` and the Python SDK.

`zeroshot template show auto-research` prints the research graph with its default of ten iterations.
Supply a positive `options.iterations` integer in the run input to set the admitted loop bound. It keeps
adopted, record-only, and aborted results under `.zeroshot/research`; add `--push` to a run when every
finalized iteration should be published.
