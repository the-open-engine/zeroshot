"""Deterministic exercise of the public graph; no provider account or network needed."""

import asyncio
import json
import os
import socket
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from zeroshot import (
    Client,
    DirectTarget,
    GraphSpec,
    HostedTarget,
    InvalidRequestError,
    LocalTarget,
    RunRequest,
    RuntimePlan,
    TargetError,
)

root, repository = map(Path, sys.argv[1:])
binary = os.environ["ZEROSHOT_PYTHON_NATIVE_BINARY"]
graph = json.loads(
    (repository / "docs/assets/file-workflow/file-workflow.graph.json").read_text()
)
guide = (repository / "docs/guides/file-workflows.md").read_text()
documented_runtime = json.loads(
    guide.split('```json title="runtime.json"\n', 1)[1].split("```", 1)[0]
)


# Script substitutes implement the authored agent instructions deterministically.
def script_graph(value):
    if isinstance(value, dict):
        value.pop("instructions", None)
        for item in value.values():
            script_graph(item)
    elif isinstance(value, list):
        for item in value:
            script_graph(item)


script_graph(graph)
runtime = {
    **documented_runtime,
    "activeMemory": {"repository": False, "graph": False},
    "globalAgents": {"postmortem": {"enabled": False}},
    "nodes": {
        "produce": {
            "kind": "script",
            "command": [
                str(Path(sys.executable).resolve()),
                "-c",
                """
import json, os
from pathlib import Path
import sys
source = Path(json.load(sys.stdin)['source'])
assert source.is_relative_to(Path(os.environ['ZEROSHOT_INPUTS']))
assert source.joinpath('data.txt').read_text() == 'before'
assert Path('source/data.txt').read_text() == 'before'
Path('report').mkdir()
Path('report/draft.txt').write_text('before')
print('null')
""",
            ],
        },
        "approval": {"kind": "request"},
        "edit": {
            "kind": "script",
            "command": [
                str(Path(sys.executable).resolve()),
                "-c",
                """
import json, os
from pathlib import Path
import sys
reply = Path(json.load(sys.stdin)['reply'])
assert reply.is_relative_to(Path(os.environ['ZEROSHOT_INPUTS']))
Path('report/draft.txt').write_text('before\\n' + reply.read_text())
print(json.dumps({'folder': 'report'}))
""",
            ],
        },
    },
}


def cli(*args, state=None, json_output=True):
    environment = dict(os.environ)
    if state:
        environment["ZEROSHOT_STATE_DIR"] = str(state)
    result = subprocess.run(
        [binary, *map(str, args)],
        env=environment,
        capture_output=True,
        text=True,
        timeout=90,
    )
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout) if json_output else result.stdout


async def settled(run, phase):
    async with asyncio.timeout(90):
        while True:
            status = await run.status()
            if status.phase == phase:
                return status
            assert status.phase != "finished", status
            await asyncio.sleep(0.03)


async def download(client, publication, destination):
    address = publication["address"]
    return await client.download_file(
        address["path"], destination, run_id=address["scope"]["runId"]
    )


async def local(managed):
    label = "managed" if managed else "in-place"
    work, state = root / label, root / ("state-" + label)
    work.mkdir()
    source = work / "source"
    source.mkdir()
    source.joinpath("data.txt").write_text("before")
    answer = root / ("answer-" + label)
    answer.write_text("after")
    async with Client(target=LocalTarget(workspace=work, state_dir=state)) as client:
        if managed:
            uploaded = await client.upload_file(
                source, "examples/source", operation="source-upload"
            )
            assert uploaded == cli(
                "file",
                "upload",
                source,
                "examples/source",
                "--operation",
                "source-upload",
                state=state,
            )
        staged = await client.stage_file(source)
        run = await client.submit(
            RunRequest(
                title="Public file workflow",
                graph=GraphSpec.from_dict(graph),
                runtime=RuntimePlan.from_dict(runtime),
                initial_input={"text": "Supply an edit file.", "source": staged},
                workspace=True,
                workspace_seeds=("user:examples/source",) if managed else (),
            )
        )
        status = await settled(run, "suspended")
        assert status.request.artifacts == {}, (
            "status-only readers must not receive attachments"
        )
        request = await run.request()
        attachment = request.request.artifacts["draft"]
        assert attachment["kind"] == ("managed" if managed else "local")
        if managed:
            destination = root / "request-before"
            assert await download(client, attachment, destination) == {
                "output": str(destination)
            }
            assert destination.joinpath("draft.txt").read_text() == "before"
            source.joinpath("data.txt").write_text("replacement")
            await client.upload_file(source, "examples/source")
        reply = await client.stage_file(answer)
        answer.write_text("changed after staging")
        continued = await run.answer({"signals": {}, "output": {"reply": reply}})
        result = await continued.wait(wait_timeout=90)
        result.raise_for_failure()
        assert result.output is None
        publication = result.artifacts["result"]
        final_status = await continued.status()
        native_status = cli("status", run.id, state=state)
        assert native_status["status"]["metadata"]["artifacts"] == result.artifacts
        if managed:
            assert publication["address"]["scope"]["runId"] == result.part_id != run.id
            assert not Path(final_status.workspace["localDirectory"]).exists()
            assert not work.joinpath("report").exists()
            destination = root / "final"
            await download(client, publication, destination)
            assert destination.joinpath("draft.txt").read_text() == "before\nafter"
            old = root / "request-after"
            await download(client, attachment, old)
            assert old.joinpath("draft.txt").read_text() == "before"
            assert (await client.list_files(run_id=result.part_id)) == cli(
                "file", "list", "--run", result.part_id, state=state
            )
            cleanup = await client.cleanup(all=True)
            assert cleanup["publications"] == 2, cleanup
            assert (await client.cleanup(all=True))["publications"] == 0
            try:
                await download(client, publication, root / "cleaned")
                raise AssertionError("cleaned publication remained downloadable")
            except TargetError as error:
                assert "not found" in str(error), error
        else:
            assert publication["kind"] == "local" and publication["path"] == "report"
            # Windows may retain its native extended-length spelling in a reference.
            assert Path(publication["root"]).samefile(work)
            assert work.joinpath("report/draft.txt").read_text() == "before\nafter"
            work.joinpath("report/draft.txt").write_text("live edit")
            assert (
                Path(attachment["root"], attachment["path"], "draft.txt").read_text()
                == "live edit"
            )
            assert (await client.list_files(run_id=result.part_id))["paths"] == []
            assert (await client.cleanup(all=True))["publications"] == 0
            assert work.joinpath("report/draft.txt").read_text() == "live edit"


async def wait_for_target(client, server):
    async with asyncio.timeout(30):
        while True:
            try:
                await client.list_files()
                return
            except TargetError:
                assert server.poll() is None, "target exited; inspect target.log"
                await asyncio.sleep(0.05)


async def direct():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    origin = f"http://127.0.0.1:{port}"
    with open(root / "target.log", "w+") as log:
        server = subprocess.Popen(
            [
                binary,
                "target",
                "serve",
                "--listen",
                f"127.0.0.1:{port}",
                "--public-origin",
                origin,
                "--storage",
                str(root / "target-state"),
            ],
            stdout=log,
            stderr=log,
        )
        try:
            cli(
                "target",
                "add",
                "file-fixture",
                "--url",
                origin,
                "--direct",
                json_output=False,
            )
            async with Client(target=HostedTarget("file-fixture")) as client:
                await wait_for_target(client, server)
                source = root / "direct-source"
                source.mkdir()
                source.joinpath("bytes").write_bytes(b"\x00\xff")
                receipt = await client.upload_file(
                    source, "data", operation="direct-upload"
                )
                assert receipt == cli(
                    "file",
                    "upload",
                    source,
                    "data",
                    "--operation",
                    "direct-upload",
                    "--target",
                    "file-fixture",
                )
                assert (await client.stage_file(source))["kind"] == "staged"
                destination = root / "direct-download"
                await client.download_file("data", destination)
                assert destination.joinpath("bytes").read_bytes() == b"\x00\xff"
                await client.remove_file("data")
                try:
                    await client.download_file("data", root / "missing")
                    raise AssertionError("missing file remained downloadable")
                except (InvalidRequestError, TargetError) as error:
                    assert "not found" in str(error), error
        finally:
            server.terminate()
            server.wait(timeout=20)


async def main():
    validate_documented_runtime()
    await local(True)
    await local(False)
    await direct()
    await unsupported()
    public_example()


async def unsupported():
    discovery = (root / "legacy-discovery.json").read_bytes()
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(discovery)))
            self.end_headers()
            self.wfile.write(discovery)

        def log_message(self, *_):
            pass

    with HTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            async with Client(
                target=DirectTarget(f"http://127.0.0.1:{server.server_port}")
            ) as client:
                try:
                    await client.list_files()
                    raise AssertionError("old host accepted file operations")
                except TargetError as error:
                    assert "file" in str(error).lower(), error
            assert requests and all("files" not in path for path in requests), requests
        finally:
            server.shutdown()
            thread.join()


def public_example():
    example = root / "public-example"
    source = example / "source"
    source.mkdir(parents=True)
    source.joinpath("data.txt").write_text("before")
    example.joinpath("answer.txt").write_text("after")
    example.joinpath("graph.json").write_text(json.dumps(graph))
    example.joinpath("runtime.json").write_text(json.dumps(runtime))
    environment = dict(os.environ, ZEROSHOT_STATE_DIR=str(example / "state"))
    output = subprocess.run(
        [
            sys.executable,
            str(repository / "sdks/python/examples/file_workflow.py"),
            "--graph",
            str(example / "graph.json"),
            "--runtime",
            str(example / "runtime.json"),
            "--source",
            str(source),
            "--answer",
            str(example / "answer.txt"),
            "--output",
            str(example / "download"),
        ],
        env=environment,
        capture_output=True,
        text=True,
        timeout=90,
    )
    assert output.returncode == 0, output.stderr
    assert example.joinpath("download/draft.txt").read_text() == "before\nafter"


def validate_documented_runtime():
    example = root / "documented-runtime"
    source = example / "source"
    source.mkdir(parents=True)
    example.joinpath("runtime.json").write_text(json.dumps(documented_runtime))
    example.joinpath("input.json").write_text(
        json.dumps({"text": "Inspect the draft.", "source": str(source)})
    )
    assert cli(
        "run",
        "--title",
        "Documented file workflow",
        "--workspace",
        "--validate-only",
        "--graph",
        repository / "docs/assets/file-workflow/file-workflow.graph.json",
        "--runtime-config",
        example / "runtime.json",
        "--input",
        example / "input.json",
    ) == {"valid": True}


asyncio.run(main())
