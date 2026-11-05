---
description: 'Use the typed asynchronous zeroshot Python SDK with local, direct and hosted targets. Install a platform wheel and submit a run.'
---

# Python SDK

`the-open-engine-zeroshot` is a typed asynchronous client for local, direct, and named hosted
targets. Each platform wheel contains the corresponding Zeroshot executable, which keeps the client
and engine on one release line.

## Install and run

Install from PyPI with Python 3.11 or newer. Pip selects the wheel for your OS and CPU:

```console
python -m pip install the-open-engine-zeroshot
```

Supported platforms are Linux x64/ARM64, macOS Intel/Apple silicon, and Windows x64.
Import the package as `zeroshot`:

```python
import asyncio

from zeroshot import Client, UniformRuntime


async def main() -> None:
    runtime = UniformRuntime(
        harness="codex",
        provider="openai",
        model="PROVIDER_MODEL_ID",
        effort="high",
    )

    async with Client(runtime=runtime) as client:
        result = await client.run(
            "Add JSON status output and cover it with focused tests.",
            cwd="/path/to/folder",
            wait_timeout=21_600,
        )

    result.raise_for_failure()


if __name__ == "__main__":
    asyncio.run(main())
```

Opening a client captures the current folder; the default preset is `Preset("software-change")`.
An omitted delivery selector uses the native template default: a PR for `software-change`, a comment
for `pr-review`, `code-review` and `issue-review`. PR publication requires Git and credentials for a
[supported forge](forge-delivery.md). Local delivery can reuse native `gh` or `glab` login.
Use `Preset("software-change", delivery="none")` for local changes without publication.
Per-run `cwd=` paths resolve from that captured folder, or `LocalTarget.workspace` when supplied.
Local graphs without Git delivery accept plain folders. Push needs committed Git and a remote.
`run()` submits once and waits; `submit()` returns a durable `Run` handle immediately.

## Watch a submitted run

```python
from zeroshot import Client, UniformRuntime


async def observe(runtime: UniformRuntime) -> None:
    async with Client(runtime=runtime) as client:
        run = await client.submit("Update the parser.")
        status = await run.status()
        async for event in run.watch(after=status.cursor):
            print(event)
```

A `RunWaitTimeout` carries the still-active run handle, and cancelling the Python task detaches in
the same way. Neither action stops native work; only `await run.force_stop()` changes run lifetime.

## Run on a direct target

Local execution uses `LocalTarget`. Docker/direct and hosted targets require a committed Git
source, selected from the client workspace or per-run `repository=`, `branch=`, and `revision=`.
`repository=` accepts a credential-free HTTPS/SSH Git URL or GitHub `owner/name` shorthand;
`source_connection=` selects a named connection used only for Git checkout and delivery:

```python
from zeroshot import Client, DirectTarget, UniformRuntime


async def run_direct(runtime: UniformRuntime) -> None:
    target = DirectTarget("http://127.0.0.1:8080")

    async with Client(target=target, runtime=runtime) as client:
        result = await client.run("Update the parser.")

    result.raise_for_failure()
```

## Submit a hosted stack

Log in with the CLI first, then pass that target's local name to `HostedTarget`. A stack is an
atomic, immutable DAG of runs over one explicit repository, base branch, and hosted profile,
submitted at once. `delivery` chooses how it lands:

- `"merge"`: the profile contains exactly one Git merge delivery node
  (`builtin.git-delivery.merge@2` or `@3`). Each run merges into the base branch before its
  dependents start, and a run may need several others.
- `"stacked_pull_requests"`: the profile contains exactly one `builtin.git-delivery.pr@2` node.
  Zeroshot turns the runs into one linear stack: each run needs exactly the run before it, in
  dependency order with ties broken by run name. Each run opens a pull request against the previous
  run's branch, the first against the base branch, and nothing is merged.

In both modes, agent bindings must not declare `GH_TOKEN`; only the Git delivery binding may.

```python
from datetime import UTC, datetime, timedelta

from zeroshot import Client, HostedTarget, StackRequest, StackRun


async def submit_release_stack() -> None:
    request = StackRequest(
        title="Release checkout update",
        repository="the-open-engine/zeroshot",
        branch="main",
        profile="org:software-change",
        delivery="merge",
        expires_at=(datetime.now(UTC) + timedelta(days=1)).isoformat(),
        runs={
            "backend": StackRun(input={"task": "Update the API."}),
            "frontend": StackRun(input={"task": "Update the client."}),
            "integrate": StackRun(
                input={"task": "Verify the combined changes and run the release tests."},
                needs=("backend", "frontend"),
            ),
        },
        submission_key="checkout-release-1",
    )

    async with Client(target=HostedTarget("platform")) as client:
        stack = await client.submit_stack(request)
        status = await stack.wait(wait_timeout=21_600)

    if not status.succeeded:
        raise RuntimeError(f"stack ended as {status.state}")
```

With a pull-request delivery profile and `delivery="stacked_pull_requests"`, the same runs become
the chain `backend`, `frontend`, `integrate`: three pull requests, each targeting the branch of the
one before it.

Submission is atomic, and every run receives a stable run ID. `needs` gates readiness only; stack
runs have no output interpolation or shared mutable input. Once its dependencies are delivered,
Platform materializes the run against an exact source branch and revision. When materialization
completes, `readyAt` is set and the run's queue deadline is the earlier of stack expiry and seven
days after `readyAt`. With merge delivery, a merge conflict is repaired by the conflicting run
itself in the same checkout and delivery loop. A descendant cannot repair a failed predecessor; it
ends as `dependency_failed`. The v1 API has no aggregate event stream, so `Stack.watch()` polls and
yields changed snapshots. [Submit a stack](stacks.md) covers delivery modes and reviewing a stacked
pull-request chain.

## Pass exact protocol documents

`GraphSpec` and `RuntimePlan` wrap exact JSON mappings without loss. Python neither copies the Rust
schemas nor applies its own semantic checks:

```python
from collections.abc import Mapping
from typing import Any

from zeroshot import Client, GraphSpec, RunRequest, RuntimePlan


async def submit_exact(
    client: Client,
    graph_document: Mapping[str, Any],
    runtime_document: Mapping[str, Any],
) -> None:
    request = RunRequest(
        title="Repair checkout",
        graph=GraphSpec.from_dict(graph_document),
        initial_input={"ticket": "OE-123"},
        runtime=RuntimePlan.from_dict(runtime_document),
        submission_key="oe-123-attempt-1",
    )

    result = await client.run(request)
    result.raise_for_failure()
```

An opaque `RuntimePlan` may carry [per-node lanes](../reference/runtime-plan.md#per-node-lanes) and
passes through unchanged, while `UniformRuntime` applies one lane to every agent node.

Before starting a local controller or contacting a direct target, the bundled executable performs
the same preflight as the CLI.

The [Python API reference](../reference/python.md) documents the public client surface.
