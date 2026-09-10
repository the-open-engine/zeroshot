"""Submit one atomic merge-only DAG through a configured hosted target."""

import asyncio
from datetime import UTC, datetime, timedelta

from zeroshot import Client, HostedTarget, MergePlanRequest, MergePlanRun


async def main() -> None:
    """Submit and wait for the example hosted merge plan."""
    request = MergePlanRequest(
        title="Release checkout update",
        repository="the-open-engine/zeroshot",
        branch="main",
        profile="org:software-change",
        expires_at=(datetime.now(UTC) + timedelta(days=1)).isoformat(),
        runs={
            "backend": MergePlanRun(input={"task": "Update the API."}),
            "frontend": MergePlanRun(input={"task": "Update the client."}),
            # Predecessors repair their own delivery conflicts before this node can start.
            "integrate": MergePlanRun(
                input={"task": "Verify the combined changes and run the release tests."},
                needs=("backend", "frontend"),
            ),
        },
        submission_key="checkout-release-1",
    )

    async with Client(target=HostedTarget("cloud")) as client:
        plan = await client.submit_plan(request)
        status = await plan.wait(wait_timeout=21_600)

    if not status.succeeded:
        raise RuntimeError(f"merge plan ended as {status.state}")


if __name__ == "__main__":
    asyncio.run(main())
