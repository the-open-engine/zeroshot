"""Submit the public file workflow using a caller-selected runtime and target."""

from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path

from zeroshot import (
    Client,
    GraphSpec,
    HostedTarget,
    RunRequest,
    RunSuspendedError,
    RuntimePlan,
)


async def main() -> None:
    """Publish a draft, supply a filepath answer, and retrieve the final folder."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--graph", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--answer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--target", help="Registered direct or capable hosted target; omit for local"
    )
    args = parser.parse_args()
    graph = GraphSpec.from_dict(json.loads(args.graph.read_text()))
    runtime = RuntimePlan.from_dict(json.loads(args.runtime.read_text()))
    async with Client(target=HostedTarget(args.target) if args.target else None) as client:
        await client.upload_file(args.source, "examples/source")
        source = await client.stage_file(args.source)
        run = await client.submit(
            RunRequest(
                title="Edit a published folder",
                graph=graph,
                runtime=runtime,
                initial_input={
                    "text": "Inspect the draft and supply an edit file.",
                    "source": source,
                },
                workspace=True,
                workspace_seeds=("user:examples/source",),
            )
        )
        try:
            await run.wait()
        except RunSuspendedError:
            request = await run.request()
            print(json.dumps(request.request.artifacts))
        answer = await client.stage_file(args.answer)
        continued = await run.answer({"signals": {}, "output": {"reply": answer}})
        result = await continued.wait()
        result.raise_for_failure()
        publication = result.artifacts["result"]
        assert isinstance(publication, dict) and publication["kind"] == "managed"
        address = publication["address"]
        assert isinstance(address, dict) and isinstance(address["path"], str)
        scope = address["scope"]
        assert isinstance(scope, dict) and isinstance(scope["runId"], str)
        print(await client.download_file(address["path"], args.output, run_id=scope["runId"]))


if __name__ == "__main__":
    asyncio.run(main())
