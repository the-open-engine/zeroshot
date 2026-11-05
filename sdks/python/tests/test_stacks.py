"""Hosted stack value-model tests."""

import pytest

from zeroshot import StackDelivery, StackRequest, StackRun, StackWaitTimeout


def test_stack_run_defensively_copies_authored_input() -> None:
    authored_input = {"task": {"labels": ["initial"]}}

    run = StackRun(input=authored_input)
    authored_input["task"]["labels"].append("mutated")

    assert run.to_dict() == {"input": {"task": {"labels": ["initial"]}}}


@pytest.mark.parametrize("delivery", ["merge", "stacked_pull_requests"])
def test_stack_request_manifest_carries_the_delivery_mode(delivery: StackDelivery) -> None:
    request = StackRequest(
        title="Release",
        repository="owner/repo",
        branch="main",
        profile="org:software-change",
        delivery=delivery,
        expires_at="2026-09-11T00:00:00Z",
        runs={"build": StackRun(input={"task": "build"})},
    )

    manifest = request.to_dict()

    assert manifest["schema"] == "zeroshot.stack/v1"
    assert manifest["delivery"] == delivery


def test_stack_wait_timeout_carries_the_durable_handle() -> None:
    class Handle:
        id = "stack-1"

    handle = Handle()
    error = StackWaitTimeout(handle, 1.5)

    assert error.stack is handle
    assert error.wait_timeout == 1.5
    assert str(error) == "stack stack-1 did not finish within 1.5 seconds"
