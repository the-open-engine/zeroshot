"""SDK resume defaults and native endpoint identity."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from conftest import read_invocations

from zeroshot import Client, DirectTarget, HostedTarget, LocalTarget


@pytest.mark.parametrize("absolute", [True, False])
def test_local_state_directory_preserves_native_endpoint_spelling(
    absolute: bool,
    fake_native: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.chdir(tmp_path)
    selected = Path("state") / ".." / "state"
    expected = Path.cwd() / selected
    target = LocalTarget(state_dir=expected if absolute else selected)

    async def exercise() -> None:
        async with Client(target=target) as client:
            await client.get_run("failed-run").resume()

    asyncio.run(exercise())
    invocations = read_invocations(fake_native)
    assert any(item["args"][0] == "resume" for item in invocations)
    assert all(item["stateDir"] == str(expected) for item in invocations)


@pytest.mark.parametrize("target_kind", ["local", "contained", "direct", "hosted"])
def test_resume_preserves_api_defaults_without_inspecting_status(
    target_kind: str, fake_native: Path
) -> None:
    async def scenario() -> None:
        target = {
            "local": LocalTarget(),
            "contained": LocalTarget(controller="contained"),
            "direct": DirectTarget("https://target.example"),
            "hosted": HostedTarget("prod"),
        }[target_kind]
        async with Client(target=target) as client:
            resumed = await client.get_run("original").resume()
            assert resumed.id == "original" and resumed.current_part_id == "successor"

    asyncio.run(scenario())
    calls = [entry["args"] for entry in read_invocations(fake_native)]
    assert not any(args[0] == "status" for args in calls)
    resume = next(args for args in calls if args[0] == "resume")
    assert "--api-default" in resume
    assert "--restart" not in resume
