# Zeroshot Python SDK

Install from PyPI with Python 3.11 or newer:

```console
python -m pip install the-open-engine-zeroshot
```

Import the package as `zeroshot`. Pip selects the wheel for your OS and CPU.
See the [Python SDK guide](https://zeroshot.sh/docs/current/guides/python-sdk/) for setup and examples.

Zeroshot enforces your team's QA process on coding agents as a graph you define. `zeroshot` is a
typed async client for its run engine. Each platform wheel contains the
matching `zeroshot` executable. Python supplies async calls and typed result objects; the executable
remains authoritative for graph validation, run state, template expansion, and runtime/provider
rules.

```python
import asyncio

from zeroshot import Client, UniformRuntime

async def main() -> None:
    runtime = UniformRuntime(
        harness="codex",
        provider="openrouter",
        model="PROVIDER_MODEL_ID",
        effort="max",
    )

    async with Client(runtime=runtime) as client:
        result = await client.run(
            "Implement the requested change and run the relevant tests.",
            wait_timeout=21_600,
        )

    result.raise_for_failure()


if __name__ == "__main__":
    asyncio.run(main())
```

`Client()` defaults to `LocalTarget()` and `Preset("software-change")`, but it never guesses a
harness, provider, or model. `run()` submits one durable graph run and waits for its terminal result.
`submit()` returns a `Run` immediately for status, resumable watch/log streams, waiting, or durable
force-stop control.

## Local, direct, and hosted targets

Local runs use the folder captured when the client opens. Set `LocalTarget.workspace` or per-run
`cwd=` to select another folder; relative `cwd` paths use that captured workspace. Agents write in
place. Without Git delivery, plain folders work; push needs committed Git and a remote, while PR
and merge need a supported forge (GitHub today).

```python
from zeroshot import Client, UniformRuntime


async def run_local(runtime: UniformRuntime) -> None:
    async with Client(runtime=runtime) as client:
        result = await client.run("Update the parser.", cwd="/path/to/folder")

    result.raise_for_failure()
```

The same client reaches an unauthenticated Docker deployment or another direct target by changing
only the target:

```python
from zeroshot import Client, DirectTarget, UniformRuntime


async def run_direct(runtime: UniformRuntime) -> None:
    target = DirectTarget("http://127.0.0.1:8080")

    async with Client(target=target, runtime=runtime) as client:
        result = await client.run("Inspect the repository and report success.")

    result.raise_for_failure()
```

Docker/direct and hosted targets require a committed Git source. Per-run `repository=` accepts a
credential-free HTTPS/SSH Git URL or GitHub `owner/name`; `branch=` and `revision=` override source
selection. `source_connection=` selects a named connection used only for checkout and delivery.

`HostedTarget` reuses a named target and login from the CLI. Merge plans are immutable DAGs over one
repository, branch, and hosted profile. That profile must contain exactly one
`builtin.git-delivery.merge@2` node; pull-request delivery isn't accepted for plans. Agent bindings
must not declare `GH_TOKEN`; only the Git delivery binding may declare it.

```python
from datetime import UTC, datetime, timedelta

from zeroshot import Client, HostedTarget, MergePlanRequest, MergePlanRun


async def run_plan() -> None:
    request = MergePlanRequest(
        title="Release checkout update",
        repository="the-open-engine/zeroshot",
        branch="main",
        profile="org:software-change",
        expires_at=(datetime.now(UTC) + timedelta(days=1)).isoformat(),
        runs={
            "backend": MergePlanRun(input={"task": "Update the API."}),
            "frontend": MergePlanRun(input={"task": "Update the client."}),
            "integrate": MergePlanRun(
                input={"task": "Verify the combined changes and run the release tests."},
                needs=("backend", "frontend"),
            ),
        },
        submission_key="checkout-release-1",
    )

    async with Client(target=HostedTarget("platform")) as client:
        plan = await client.submit_plan(request)
        status = await plan.wait(wait_timeout=21_600)

    if not status.succeeded:
        raise RuntimeError(f"merge plan ended as {status.state}")
```

Plan input is static JSON. A dependency controls when a node may start; it doesn't copy output into
another node. Run IDs are assigned once during atomic submission. After dependencies merge, Platform
materializes a node against an exact source revision. When materialization completes, `readyAt` is
set and the node's queue deadline is the earlier of plan expiry and seven days after `readyAt`. A
merge conflict is repaired by the conflicting run itself in the same checkout and delivery loop. A
descendant cannot repair a failed predecessor; it ends as `dependency_failed`. `MergePlan.watch()`
polls the aggregate Platform status and emits changed snapshots because the v1 plan API has no
streaming endpoint.

## Exact graph and runtime control

Use `GraphSpec` and `RuntimePlan` to pass exact JSON values unchanged. Python does not traverse or
validate either document:

```python
from collections.abc import Mapping
from typing import Any

from zeroshot import GraphSpec, RunRequest, RuntimePlan


def exact_request(
    graph_document: Mapping[str, Any],
    runtime_document: Mapping[str, Any],
) -> RunRequest:
    return RunRequest(
        title="Repair checkout",
        graph=GraphSpec.from_dict(graph_document),
        initial_input={"ticket": "OE-123"},
        runtime=RuntimePlan.from_dict(runtime_document),
        submission_key="oe-123-attempt-1",
    )
```

`await client.list_presets()` and `await client.get_preset(...)` query the bundled Rust catalog.
The Python package contains no copied preset registry, provider/model registry, graph schema, or
semantic validator. Every submission runs the executable's preflight before a local controller
starts or a direct target is contacted.

## Durable observation

`Run.watch(after=...)` and `Run.logs(after=..., execution=...)` resume strictly after opaque native
cursors. A `wait_timeout` expiry raises `RunWaitTimeout` carrying the still-active `Run`; cancelling
the Python await also detaches without stopping it. `Run.force_stop()` is the only operation that
changes run lifetime. Each `LogEvent.timestamp` is the positive, JavaScript-safe Unix epoch
millisecond captured at the producer and preserved unchanged by durable replay.

## Environment values

With `environment=None`, submission values come from `os.environ`. An explicit mapping replaces
ambient credential variables; native saved connections and local harness login still apply.
Zeroshot selects declared runtime and source-connection fields; transport credentials stay outside
agent environments unless independently declared by a node. The executable owns provider defaults.

## Packaging and versions

The package is released only as platform wheels because each wheel bundles one matching native
executable; there is no first-run download, Node dependency, PyO3 ABI, or auth dependency. The
distribution name is `the-open-engine-zeroshot`, the import package remains `zeroshot`, and
`py.typed` is included.

An SDK release tag is `zeroshot-python-vZEROSHOT_SDK`, for example
`zeroshot-python-v8.0.0_1`; its PEP 440 package version is `8.0.0.post1`. Revision `1` is released
automatically after the corresponding Zeroshot release. Later SDK-only revisions may be released
independently against the same immutable Zeroshot release.
