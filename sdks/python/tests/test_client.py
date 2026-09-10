"""Public client contract tests."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from conftest import read_invocations

from zeroshot import (
    Client,
    ClientClosedError,
    DirectTarget,
    GraphSpec,
    HostedTarget,
    InvalidRequestError,
    LocalTarget,
    MergePlanRequest,
    MergePlanRun,
    RunFailedError,
    RunRequest,
    RunResult,
    RuntimePlan,
    RunWaitTimeout,
    TargetError,
    UniformRuntime,
)


def runtime() -> UniformRuntime:
    return UniformRuntime(
        harness="codex",
        provider="openrouter",
        model="gpt-5.6-luna",
        effort="max",
    )


def test_uniform_runtime_requires_an_explicit_harness() -> None:
    with pytest.raises(TypeError, match="harness"):
        UniformRuntime(provider="openrouter", model="gpt-5.6-luna")  # type: ignore[call-arg]


def direct_client(fake_native: Path) -> Client:
    return Client(
        target=DirectTarget("http://127.0.0.1:8080"),
        runtime=runtime(),
        environment={
            "OPENROUTER_API_KEY": "test-only",
            "FAKE_ZEROSHOT_LOG": str(fake_native),
            "FAKE_LIST_ONE": "1",
        },
    )


def test_run_waits_for_terminal_result(fake_native: Path, tmp_path: Path) -> None:
    async def exercise() -> None:
        async with Client(target=LocalTarget(tmp_path), runtime=runtime()) as client:
            result = await client.run("change it", wait_timeout=2)
            assert result.succeeded
            assert result.output == {"ok": True}
            result.raise_for_failure()

    asyncio.run(exercise())
    invocations = read_invocations(fake_native)
    runs = [item for item in invocations if item["args"][0] == "run"]
    assert len(runs) == 2
    assert all(item["--input"] == {"task": "change it"} for item in runs)
    assert all(
        item["--uniform-runtime-config"]
        == {
            "harness": "codex",
            "provider": "openrouter",
            "model": "gpt-5.6-luna",
            "effort": "max",
            "size": "medium",
            "sessionScope": "execution",
        }
        for item in runs
    )
    keys = [item["args"][item["args"].index("--submission-key") + 1] for item in runs]
    assert keys[0] == keys[1]
    assert "--validate-only" in runs[0]["args"]
    assert "--detach" in runs[1]["args"]
    watch = next(item["args"] for item in invocations if item["args"][0] == "watch")
    assert watch[watch.index("--after") + 1] == "v2:0"


def test_direct_target_uses_the_same_client_and_durable_run_surface(fake_native: Path) -> None:
    async def exercise() -> None:
        async with direct_client(fake_native) as client:
            run = await client.submit(
                "inspect it",
                repository="owner/repo",
                branch="main",
                revision="0123456789abcdef0123456789abcdef01234567",
            )
            status = await run.status()
            assert status.phase == "admitted"
            summaries = await client.list_runs()
            assert len(summaries) == 1
            assert summaries[0].phase == "admitted"
            logs = [event async for event in run.logs(after="v2:2", execution="worker-1")]
            assert [(event.execution, event.message) for event in logs] == [("worker-1", "done")]
            assert logs[0].timestamp == 1_725_000_000_123
            result = await run.force_stop()
            assert not result.succeeded
            assert result.failure == "force_stopped"

    asyncio.run(exercise())
    arguments = [item["args"] for item in read_invocations(fake_native)]
    assert arguments[0][0] == "run"
    assert "--validate-only" in arguments[0]
    assert arguments[1][:2] == ["target", "add"]
    assert all(args[:2] != ["target", "setup"] for args in arguments)
    run = next(args for args in arguments if args[0] == "run" and "--detach" in args)
    assert run[run.index("--target") : run.index("--target") + 2] == ["--target", "python-sdk"]
    assert run[run.index("--repository") : run.index("--repository") + 2] == [
        "--repository",
        "owner/repo",
    ]
    assert run[run.index("--branch") : run.index("--branch") + 2] == ["--branch", "main"]
    assert run[run.index("--revision") : run.index("--revision") + 2] == [
        "--revision",
        "0123456789abcdef0123456789abcdef01234567",
    ]
    logs = next(args for args in arguments if args[0] == "logs")
    assert logs[logs.index("--after") + 1] == "v2:2"
    assert logs[logs.index("--execution") + 1] == "worker-1"


def test_uniform_runtime_encodes_exact_connection_requirements() -> None:
    selected = UniformRuntime(
        harness="codex",
        provider="openai",
        model="gpt-5.6-luna",
        connections={"subscription": ("SESSION_MARKER",)},
    )

    assert selected.to_dict() == {
        "harness": "codex",
        "provider": "openai",
        "model": "gpt-5.6-luna",
        "size": "medium",
        "sessionScope": "execution",
        "connections": {"subscription": ["SESSION_MARKER"]},
    }


def test_custom_graph_and_runtime_are_forwarded_unchanged(fake_native: Path) -> None:
    graph = {"profile": "openengine.graph.full/v1", "root": {"kind": "succeed"}}
    runtime_plan = {"harness": "codex", "provider": "openai", "size": "tiny", "nodes": {}}
    request = RunRequest(
        title="Exact request",
        graph=GraphSpec.from_dict(graph),
        initial_input={"ticket": "OE-123"},
        runtime=RuntimePlan.from_dict(runtime_plan),
        repository="owner/repository",
        branch="release",
        revision="0123456789abcdef0123456789abcdef01234567",
        submission_key="oe-123",
    )

    async def exercise() -> None:
        async with Client() as client:
            await client.submit(request)

    asyncio.run(exercise())
    runs = [
        item for item in read_invocations(fake_native) if item["args"] and item["args"][0] == "run"
    ]
    assert len(runs) == 2
    for item in runs:
        assert item["--input"] == {"ticket": "OE-123"}
        assert item["--graph"] == graph
        assert item["--runtime-config"] == runtime_plan
        assert "--uniform-runtime-config" not in item["args"]
        arguments = item["args"]
        assert arguments[arguments.index("--repository") + 1] == "owner/repository"
        assert arguments[arguments.index("--branch") + 1] == "release"
        assert (
            arguments[arguments.index("--revision") + 1]
            == "0123456789abcdef0123456789abcdef01234567"
        )


def test_presets_are_read_from_executable(fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client() as client:
            assert await client.list_presets() == ("single-worker", "software-change")
            graph = await client.get_preset("software-change", delivery="pull_request")
            assert graph.document["name"] == "software-change"
            assert graph.document["delivery"] == "pull_request"

    asyncio.run(exercise())
    arguments = [item["args"] for item in read_invocations(fake_native)]
    assert ["template", "list"] in arguments
    assert [
        "template",
        "show",
        "software-change",
        "--delivery",
        "pull_request",
    ] in arguments


def test_native_validation_error_is_structured_and_redacted(fake_native: Path) -> None:
    secret = "must-not-escape"

    async def exercise() -> None:
        async with Client(
            runtime=runtime(),
            environment={
                "OPENROUTER_API_KEY": secret,
                "FAKE_ZEROSHOT_LOG": str(fake_native),
            },
        ) as client:
            with pytest.raises(InvalidRequestError) as caught:
                await client.submit("invalid")
            assert caught.value.code == "runtime.missing_binding"
            assert caught.value.node == "worker"
            assert secret not in str(caught.value)
            assert "<redacted>" in str(caught.value)

    asyncio.run(exercise())


def test_invalid_direct_run_never_contacts_target(fake_native: Path) -> None:
    async def exercise() -> None:
        async with direct_client(fake_native) as client:
            with pytest.raises(InvalidRequestError):
                await client.submit("invalid")

    asyncio.run(exercise())
    arguments = [item["args"] for item in read_invocations(fake_native)]
    assert len(arguments) == 1
    assert arguments[0][0] == "run"
    assert "--validate-only" in arguments[0]


def test_wait_timeout_carries_run_and_does_not_force_stop(fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client(
            runtime=runtime(),
            environment={
                "FAKE_WATCH_DELAY": "1",
                "FAKE_ZEROSHOT_LOG": str(fake_native),
                "OPENROUTER_API_KEY": "test-only",
            },
        ) as client:
            run = await client.submit("slow")
            with pytest.raises(RunWaitTimeout) as caught:
                await run.wait(wait_timeout=0.01)
            assert caught.value.run is run

    asyncio.run(exercise())
    assert not any(item["args"][0] == "force-stop" for item in read_invocations(fake_native))


def test_missing_runtime_fails_before_native_preflight(fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(InvalidRequestError) as caught:
                await client.submit("missing runtime")
            assert caught.value.code == "runtime.required"

    asyncio.run(exercise())
    assert read_invocations(fake_native) == []


def test_closed_client_rejects_run_handle(fake_native: Path) -> None:
    async def exercise() -> None:
        client = Client()
        run = client.get_run("01900000-0000-7000-8000-000000000001")
        await client.aclose()
        with pytest.raises(ClientClosedError):
            await run.status()

    asyncio.run(exercise())


def test_failed_result_has_opt_in_exception_projection() -> None:
    result = RunResult(run_id="run-1", succeeded=False, failure="worker_failed")
    with pytest.raises(RunFailedError) as caught:
        result.raise_for_failure()
    assert caught.value.result is result


def test_hosted_plan_submission_uses_one_strict_manifest(fake_native: Path) -> None:
    request = MergePlanRequest(
        title="Release",
        repository="owner/repo",
        branch="main",
        profile="org:software-change",
        expires_at="2026-09-11T00:00:00Z",
        runs={
            "build": MergePlanRun(input={"task": "build"}),
            "integrate": MergePlanRun(input={"task": "integrate"}, needs=("build",)),
        },
        submission_key="release-1",
    )

    async def exercise() -> None:
        async with Client(target=HostedTarget("cloud")) as client:
            plan = await client.submit_plan(request)
            assert plan.id == "01900000-0000-7000-8000-000000000010"

    asyncio.run(exercise())
    invocations = read_invocations(fake_native)
    assert [item["args"][:2] for item in invocations] == [
        ["plan", "validate"],
        ["plan", "submit"],
    ]
    expected = {
        "schema": "zeroshot.merge-plan/v1",
        "title": "Release",
        "source": {"repository": "owner/repo", "branch": "main"},
        "profile": "org:software-change",
        "expiresAt": "2026-09-11T00:00:00Z",
        "runs": {
            "build": {"input": {"task": "build"}},
            "integrate": {"input": {"task": "integrate"}, "needs": ["build"]},
        },
    }
    assert all(item["plan"] == expected for item in invocations)
    submitted = invocations[1]["args"]
    assert submitted[submitted.index("--target") + 1] == "cloud"
    assert submitted[submitted.index("--submission-key") + 1] == "release-1"
    assert "--detach" in submitted


def test_hosted_plan_handle_observes_waits_and_force_stops(fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client(target=HostedTarget("cloud")) as client:
            plan = client.get_plan("01900000-0000-7000-8000-000000000010")
            status = await plan.status()
            assert status.state == "queued"
            assert status.runs[0].waiting_reason == "queue_capacity"
            snapshots = [item async for item in plan.watch()]
            assert [item.state for item in snapshots] == ["running", "succeeded"]
            terminal = await plan.wait(wait_timeout=2)
            assert terminal.succeeded
            stopped = await plan.force_stop()
            assert stopped.state == "cancelled"

    asyncio.run(exercise())
    arguments = [item["args"] for item in read_invocations(fake_native)]
    assert all(args[args.index("--target") + 1] == "cloud" for args in arguments)
    assert any(args[:2] == ["plan", "status"] for args in arguments)
    assert any(args[:2] == ["plan", "watch"] for args in arguments)
    assert any(args[:2] == ["plan", "force-stop"] for args in arguments)


@pytest.mark.parametrize("state", ["failed", "cancelled", "expired"])
def test_hosted_plan_watch_accepts_terminal_status_before_cli_failure(
    fake_native: Path,
    monkeypatch: pytest.MonkeyPatch,
    state: str,
) -> None:
    monkeypatch.setenv("FAKE_PLAN_WATCH_TERMINAL", state)

    async def exercise() -> None:
        async with Client(target=HostedTarget("cloud")) as client:
            plan = client.get_plan("01900000-0000-7000-8000-000000000010")
            statuses = [status.state async for status in plan.watch()]
            assert statuses == ["running", state]

    asyncio.run(exercise())


def test_hosted_plan_watch_propagates_preterminal_cli_failure(
    fake_native: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("FAKE_PLAN_WATCH_FAIL_BEFORE_TERMINAL", "1")

    async def exercise() -> None:
        observed = []
        async with Client(target=HostedTarget("cloud")) as client:
            plan = client.get_plan("01900000-0000-7000-8000-000000000010")
            with pytest.raises(TargetError, match="failed before terminal") as caught:
                async for status in plan.watch():
                    observed.append(status.state)
        assert caught.value.exit_code == 1
        assert observed == ["running"]

    asyncio.run(exercise())


def test_hosted_plan_watch_propagates_unexpected_failure_after_success(
    fake_native: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("FAKE_PLAN_WATCH_EXIT_NONZERO", "1")

    async def exercise() -> None:
        observed = []
        async with Client(target=HostedTarget("cloud")) as client:
            plan = client.get_plan("01900000-0000-7000-8000-000000000010")
            with pytest.raises(TargetError, match="finished unsuccessfully"):
                async for status in plan.watch():
                    observed.append(status.state)
        assert observed == ["running", "succeeded"]

    asyncio.run(exercise())


def test_merge_plan_requires_a_hosted_target(fake_native: Path) -> None:
    request = MergePlanRequest(
        title="Release",
        repository="owner/repo",
        branch="main",
        profile="org:software-change",
        expires_at="2026-09-11T00:00:00Z",
        runs={"build": MergePlanRun(input={"task": "build"})},
    )

    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(InvalidRequestError) as caught:
                await client.submit_plan(request)
            assert caught.value.code == "target.hosted_required"

    asyncio.run(exercise())
    assert read_invocations(fake_native) == []
