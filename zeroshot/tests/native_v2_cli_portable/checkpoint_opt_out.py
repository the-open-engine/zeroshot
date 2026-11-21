"""Exercise the dependency-free SDK through the shipped CLI and actual target HTTP."""

import asyncio
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from zeroshot import (
    Client,
    DirectTarget,
    GraphSpec,
    InvalidRequestError,
    RunRequest,
    RuntimePlan,
    TargetError,
)

MARKER = "openengine.workspace-checkpoint-opt-out/v1"
RUN_ID = "018f5e78-7f95-7c22-8d98-3f15af20c991"
files = Path(sys.argv[1])
discovery = json.loads((files / "discovery.json").read_text())
posts = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, body):
        encoded = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self):
        assert self.path == "/.well-known/zeroshot-native-v2", self.path
        self.reply(discovery)

    def do_POST(self):
        assert self.path == "/native-v2/run", self.path
        posts.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
        self.reply({"runId": RUN_ID})


async def main():
    runtime = json.loads((files / "runtime.json").read_text())
    runtime["workspaceCheckpoints"] = False
    runtime["nodes"]["worker"]["connections"] = {"openai": ["OPENAI_API_KEY"]}
    request = RunRequest(
        title="Checkpoint policy",
        graph=GraphSpec.from_dict(json.loads((files / "graph.json").read_text())),
        runtime=RuntimePlan.from_dict(runtime),
        initial_input=None,
        repository="https://github.com/acme/native-fixture.git",
        branch="main",
        revision="1" * 40,
    )
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        for marker in (None, "openengine.workspace-checkpoint-opt-out/v2", MARKER):
            discovery["extensions"] = {
                "git_transport": {"kind": "openengine.git-transport/v1"}
            }
            if marker:
                discovery["extensions"]["workspace_checkpoint_opt_out"] = {
                    "kind": marker
                }
            async with Client(
                target=DirectTarget(f"http://127.0.0.1:{server.server_port}")
            ) as client:
                if marker == MARKER:
                    run = await client.submit(request)
                    assert run.id == RUN_ID
                else:
                    try:
                        await client.submit(request)
                    except (TargetError, InvalidRequestError) as error:
                        assert (
                            "does not support disabling workspace checkpoints"
                            in str(error)
                        ), str(error)
                    else:
                        raise AssertionError(
                            "unsupported target accepted checkpoint opt-out"
                        )
                    assert not posts
        assert len(posts) == 1, posts
        assert posts[0]["submission"]["runtime"]["workspaceCheckpoints"] is False
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


asyncio.run(main())
