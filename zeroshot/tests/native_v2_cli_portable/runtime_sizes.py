"""Local SDK preflight reports an invalid request before native submission."""

import asyncio
import json
import sys
from pathlib import Path

from zeroshot import (
    Client,
    GraphSpec,
    InvalidRequestError,
    Preset,
    RunRequest,
    RuntimePlan,
    UniformRuntime,
)


async def main():
    """Require typed preflight refusals for uniform and exact local requests."""
    graph = GraphSpec.from_dict(json.loads(Path(sys.argv[1]).read_text()))
    for size in ("medium", "small", "custom-big-box"):
        runtime = UniformRuntime(
            harness="codex", provider="openai", model="fixture-model", size=size
        )
        request = RunRequest(
            title="Invalid local size",
            graph=graph,
            runtime=RuntimePlan.from_dict(
                {
                    "harness": "codex",
                    "provider": "openai",
                    "size": size,
                    "nodes": {"worker": {"kind": "agent", "model": "fixture-model"}},
                }
            ),
            initial_input=None,
        )
        async with Client(
            preset=Preset("single-worker", delivery="none"), runtime=runtime
        ) as client:
            for task in ("Inspect this task", request):
                try:
                    await client.submit(task)
                except InvalidRequestError as error:
                    assert error.code == "request.invalid", error.code
                    assert f"does not offer run size {size}" in str(error), str(error)
                    assert "supported sizes: host" in str(error), str(error)
                else:
                    raise AssertionError("unoffered size passed local SDK preflight")


asyncio.run(main())
