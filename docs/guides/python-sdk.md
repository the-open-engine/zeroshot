# Python SDK

`the-open-engine-zeroshot` is a typed asynchronous client for local and direct targets. Each platform
wheel contains the corresponding Zeroshot executable, which keeps the client and engine on one
release line.

## Install and run

```console
python -m pip install the-open-engine-zeroshot
```

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
            wait_timeout=21_600,
        )

    result.raise_for_failure()


if __name__ == "__main__":
    asyncio.run(main())
```

`Client()` uses the current worktree and the `software-change` preset unless told otherwise. You must
still choose the harness, provider, and model. `run()` submits once and waits, whereas `submit()`
returns a durable `Run` handle immediately.

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

Local execution uses `LocalTarget`. A direct target, including the Docker image, needs its origin
and source repository:

```python
from zeroshot import Client, DirectTarget, UniformRuntime


async def run_direct(runtime: UniformRuntime) -> None:
    target = DirectTarget(
        "http://127.0.0.1:8080",
        repository="owner/repository",
        default_branch="main",
    )

    async with Client(target=target, runtime=runtime) as client:
        result = await client.run("Update the parser.")

    result.raise_for_failure()
```

The Python target contract does not include hosted authentication, so use the CLI for a hosted
target.

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

Before starting a local controller or contacting a direct target, the bundled executable performs
the same preflight as the CLI.

The [Python API reference](../reference/python.md) documents the public client surface.
