"""Submit one atomic stack of runs through a configured hosted target."""

import asyncio
from datetime import UTC, datetime, timedelta

from zeroshot import Client, HostedTarget, StackRequest, StackRun


async def main() -> None:
    """Submit and wait for the example hosted stack."""
    request = StackRequest(
        title="Release checkout update",
        repository="the-open-engine/zeroshot",
        branch="main",
        profile="org:software-change",
        # The profile has exactly one Git merge delivery node. A profile with one pull-request
        # delivery node takes delivery="stacked_pull_requests" instead: the runs form one linear
        # chain of pull requests, each targeting the previous run's branch, and nothing merges.
        delivery="merge",
        expires_at=(datetime.now(UTC) + timedelta(days=1)).isoformat(),
        runs={
            "backend": StackRun(input={"task": "Update the API."}),
            "frontend": StackRun(input={"task": "Update the client."}),
            # Predecessors repair their own delivery conflicts before this run can start.
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


if __name__ == "__main__":
    asyncio.run(main())
