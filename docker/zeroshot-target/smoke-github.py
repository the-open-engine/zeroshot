"""Exercise the installed gh GraphQL pagination contract without GitHub access."""

import http.server
import json
import os
import subprocess
import tempfile
import threading
from typing import ClassVar


class GraphQLHandler(http.server.BaseHTTPRequestHandler):
    requests: ClassVar[list[dict]] = []

    def do_POST(self):
        if self.path != "http://api.github.localhost/graphql":
            self.send_error(404)
            return
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.requests.append(request)
        page = len(self.requests)
        if page > 2 or (
            page == 2 and request.get("variables", {}).get("endCursor") != "page1"
        ):
            self.send_error(400, "Unexpected pagination cursor")
            return
        payload = json.dumps(
            {
                "data": {
                    "viewer": {
                        "repositories": {
                            "nodes": [{"id": str(page)}],
                            "pageInfo": {
                                "hasNextPage": page == 1,
                                "endCursor": f"page{page}",
                            },
                        }
                    }
                },
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass


def main():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), GraphQLHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="github-smoke-") as home:
            # gh uses HTTP for *.localhost. The proxy keeps every request on loopback,
            # including in an image with no DNS or external network access.
            environment = {
                "PATH": os.environ["PATH"],
                "HOME": home,
                "GH_CONFIG_DIR": home,
                "GH_ENTERPRISE_TOKEN": "offline-smoke-token",
                "GH_PROMPT_DISABLED": "1",
                "GH_NO_UPDATE_NOTIFIER": "1",
                "HTTP_PROXY": f"http://127.0.0.1:{server.server_port}",
                "NO_PROXY": "",
            }
            query = (
                "query($endCursor: String) { viewer { repositories(first:1, after:$endCursor) { "
                "nodes{id} pageInfo{hasNextPage endCursor} } } }"
            )
            result = subprocess.run(
                [
                    "gh",
                    "api",
                    "graphql",
                    "--hostname",
                    "github.localhost",
                    "--paginate",
                    "--slurp",
                    "-f",
                    f"query={query}",
                ],
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            assert result.returncode == 0, result.stderr
            pages = json.loads(result.stdout)
            assert len(pages) == len(GraphQLHandler.requests) == 2, pages
            assert [
                page["data"]["viewer"]["repositories"]["nodes"][0]["id"]
                for page in pages
            ] == ["1", "2"], pages
            print("GitHub CLI GraphQL --paginate --slurp: two pages, cursor advanced")
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    main()
