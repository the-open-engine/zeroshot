"""Hosted merge-plan value-model tests."""

from zeroshot import MergePlanRun


def test_merge_plan_run_defensively_copies_authored_input() -> None:
    authored_input = {"task": {"labels": ["initial"]}}

    run = MergePlanRun(input=authored_input)
    authored_input["task"]["labels"].append("mutated")

    assert run.to_dict() == {"input": {"task": {"labels": ["initial"]}}}
