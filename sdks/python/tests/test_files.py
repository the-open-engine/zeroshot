"""File operations keep the CLI route, receipts, diagnostics and cancellation semantics."""

from __future__ import annotations

import asyncio
import os
import tempfile
from pathlib import Path

import pytest
from conftest import read_invocations

from zeroshot import (
    Client,
    DirectTarget,
    GraphSpec,
    HostedTarget,
    InvalidRequestError,
    LocalTarget,
    ProtocolError,
    RunRequest,
    RuntimePlan,
    UniformRuntime,
)
from zeroshot._projection import _human_request, _status


@pytest.mark.skipif(os.name == "nt", reason="requires a directory symlink")
def test_direct_files_resolve_linked_system_temp_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_native: Path
) -> None:
    physical_root = tmp_path.resolve() / "physical"
    physical_root.mkdir()
    linked_root = tmp_path / "linked"
    linked_root.symlink_to(physical_root, target_is_directory=True)
    monkeypatch.setattr(tempfile, "tempdir", str(linked_root))

    async def exercise() -> Path:
        async with Client(target=DirectTarget("http://127.0.0.1:8080")) as client:
            assert (await client.list_files())["kind"] == "list"
            return Path(client._environment()[0]["ZEROSHOT_CONFIG_DIR"])

    config = asyncio.run(exercise())
    assert config.is_relative_to(physical_root)


@pytest.mark.parametrize(
    "target", [LocalTarget(), DirectTarget("http://127.0.0.1:8080"), HostedTarget("named")]
)
def test_file_routes_and_receipts_match_cli(target: object, fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client(target=target) as client:
            assert (
                await client.upload_file("a folder", "library/data", scope="org", operation="retry")
            )["kind"] == "replaced"
            assert await client.stage_file("a file", run_id="upload", operation="retry") == {
                "kind": "staged",
                "runId": "upload-1",
                "operation": "operation-1",
            }
            assert (await client.list_files())["kind"] == "list"
            assert (await client.list_files("result", run_id="producing-part", member="nested"))[
                "kind"
            ] == "members"
            assert await client.download_file("result", "new folder", run_id="producing-part") == {
                "output": "new folder"
            }
            assert await client.remove_file("library/data") == {"kind": "removed", "removed": True}
            assert await client.cleanup(all=True) == {
                "kind": "cleaned",
                "publications": 2,
                "checkpointLineages": 1,
            }

    asyncio.run(exercise())
    calls = [entry["args"] for entry in read_invocations(fake_native)]
    route = (
        []
        if isinstance(target, LocalTarget)
        else ["--target", "python-sdk" if isinstance(target, DirectTarget) else "named"]
    )
    assert calls[-7:] == [
        [
            "file",
            "upload",
            "--scope",
            "org",
            *route,
            "--operation",
            "retry",
            "--",
            "a folder",
            "library/data",
        ],
        ["file", "stage", "--operation", "retry", "--run-id", "upload", *route, "--", "a file"],
        ["file", "list", *route],
        ["file", "list", "--run", "producing-part", *route, "--member", "nested", "--", "result"],
        [
            "file",
            "download",
            "--run",
            "producing-part",
            *route,
            "--member",
            "",
            "--",
            "result",
            "new folder",
        ],
        ["file", "remove", "--scope", "user", *route, "--", "library/data"],
        ["cleanup", "--all", *route],
    ]


@pytest.mark.parametrize("code", ["files.unsupported", "files.not_found", "files.released"])
def test_file_refusals_remain_typed(
    code: str, fake_native: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_FILE_ERROR", code)

    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(InvalidRequestError) as caught:
                await client.download_file("missing", "new")
            assert caught.value.code == code
            with pytest.raises(InvalidRequestError, match="cannot be combined"):
                await client.list_files(scope="user", run_id="part")

    asyncio.run(exercise())


@pytest.mark.parametrize(
    "reply", ["{}", '{"kind":"staged","operation":3}', '{"kind":"removed","removed":1}']
)
def test_malformed_file_receipts_are_not_usable(
    reply: str, fake_native: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_FILE_RESPONSE", reply)

    async def exercise() -> None:
        async with Client() as client:
            with pytest.raises(ProtocolError):
                await client.stage_file("file")

    asyncio.run(exercise())


def test_workspace_options_reach_preflight_and_submission(fake_native: Path) -> None:
    async def exercise() -> None:
        async with Client(
            runtime=UniformRuntime(harness="codex", provider="openai", model="fixture")
        ) as client:
            await client.submit(
                "task",
                workspace=True,
                workspace_placement="managed",
                workspace_seeds=("org:dataset#folder",),
                cwd="work",
            )
            await client.submit(
                RunRequest(
                    title="copy",
                    graph=GraphSpec.from_dict({}),
                    runtime=RuntimePlan.from_dict({}),
                    initial_input=None,
                    workspace=True,
                    copy_workspace=True,
                    cwd="exact folder",
                )
            )

    asyncio.run(exercise())
    calls = read_invocations(fake_native)
    assert len(calls) == 4
    for call in calls[:2]:
        args = call["args"]
        assert "--workspace" in args
        assert args[args.index("--workspace-placement") + 1] == "managed"
        assert args[args.index("--workspace-seed") + 1] == "org:dataset#folder"
    for call in calls[2:]:
        assert "--workspace" in call["args"] and "--copy-workspace" in call["args"]
        assert "--workspace-seed" not in call["args"]


def test_publications_preserve_cli_metadata_and_status_redaction() -> None:
    managed = {
        "kind": "managed",
        "target": "local",
        "address": {"scope": {"kind": "run", "runId": "part"}, "path": "final/report"},
    }
    local = {"kind": "local", "root": "/work", "path": "report"}
    base = {
        "runId": "run",
        "partId": "part",
        "currentPartId": "part",
        "title": "files",
        "source": None,
        "size": "host",
        "atCursor": "v2:1",
        "workspace": {"placement": "managed"},
        "status": {
            "phase": "finished",
            "terminalResult": {"status": "succeeded", "output": None},
            "metadata": {"artifacts": {"result": managed, "live": local}},
        },
    }
    result = _status(base)
    assert result.workspace == {"placement": "managed"}
    assert result.result.artifacts == {"result": managed, "live": local}
    question = {"id": "q", "text": "?", "signals": {}, "output": {}, "expiresAt": 1000}
    assert _human_request(question).artifacts == {}
    assert _human_request({**question, "artifacts": {"draft": managed}}).artifacts == {
        "draft": managed
    }


@pytest.mark.skipif(os.name == "nt", reason="PID existence probe uses Unix signals")
def test_cancelling_a_transfer_reaps_the_cli(
    fake_native: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    pid_file = tmp_path / "pid"
    monkeypatch.setenv("FAKE_FILE_DELAY", "1")
    monkeypatch.setenv("FAKE_FILE_PID", str(pid_file))

    async def exercise() -> None:
        async with Client() as client:
            task = asyncio.create_task(client.stage_file("source"))
            async with asyncio.timeout(5):
                for _ in range(500):
                    if pid_file.exists():
                        break
                    await asyncio.sleep(0.01)
                assert pid_file.exists()
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            with pytest.raises(ProcessLookupError):
                os.kill(int(pid_file.read_text()), 0)

    asyncio.run(exercise())
