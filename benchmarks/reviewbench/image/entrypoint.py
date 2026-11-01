#!/usr/bin/env python3
"""ReviewBench adapter for the Zeroshot pull-request review graph.

Implements the ReviewBench agent contract: read the mounted pull request, run
the review graph on the checkout, and write the findings file at $RB_OUT. The
graph's own output is generic (file, line range, severity, title, body); this
adapter is the only benchmark-specific code. It writes nothing else into the
mounts and logs to stderr only.

If the graph misses its deadline or fails, the adapter returns the most refined
findings that completed nodes produced. If no reviewer completed at all, it
exits non-zero so that the benchmark retries the pull request.

Credentials come from the environment: ANTHROPIC_API_KEY for the Claude agents
and OPENAI_API_KEY for the Codex agents.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

START = time.time()
ENV = os.environ
HOME = Path(ENV.get("HOME", "/root"))
CONFIG = Path("/opt/rb/config")
REPO = Path(ENV.get("RB_REPO", "/work/repo"))
OUT = Path(ENV.get("RB_OUT", "/work/out/findings.json"))
AGENT = ENV.get("RB_AGENT", "zeroshot-code-review")
# Leaves time to stop the run and write the file inside the benchmark's 30 minutes.
DEADLINE = float(ENV.get("RB_DEADLINE_SECONDS", "1560"))
STATE = HOME / ".local" / "state" / "zeroshot"
REVIEWERS = ("unit_review_", "hunt_")


def log(message: str) -> None:
    print(f"[{time.time() - START:7.1f}s] {message}", file=sys.stderr, flush=True)


# --- input -------------------------------------------------------------------


def changed_files(diff: str) -> list[dict]:
    files: list[dict] = []
    current = None
    in_hunk = False
    for line in diff.splitlines():
        if line.startswith("diff --git "):
            match = re.match(r"diff --git a/(.*) b/(.*)$", line)
            path = match.group(2) if match else line.split(" b/", 1)[-1]
            current = {"path": path, "status": "modified", "added": 0, "removed": 0}
            files.append(current)
            in_hunk = False
        elif current is None:
            continue
        elif line.startswith("@@"):
            in_hunk = True
        elif not in_hunk:
            if line.startswith("new file mode"):
                current["status"] = "added"
            elif line.startswith("deleted file mode"):
                current["status"] = "deleted"
            elif line.startswith("rename from"):
                current["status"] = "renamed"
            elif line.startswith("Binary files"):
                current["status"] = "binary"
            elif line.startswith("+++ b/"):
                current["path"] = line[6:]
        elif line.startswith("+"):
            current["added"] += 1
        elif line.startswith("-"):
            current["removed"] += 1
    return files


def write_input(pr: dict) -> Path:
    diff_path = ENV.get("RB_DIFF", "/work/pr/diff.patch")
    diff = Path(diff_path).read_text(errors="replace")
    payload = {
        "title": pr.get("title") or "",
        "body": pr.get("body") or "",
        "repository": pr.get("nwo") or ENV.get("RB_NWO", ""),
        "baseRevision": ENV.get("RB_BASE", pr.get("base", "")),
        "headRevision": ENV.get("RB_HEAD", pr.get("head", "")),
        "diffPath": diff_path,
        "changedFiles": changed_files(diff),
    }
    path = Path(tempfile.mkdtemp(prefix="review-")) / "graph-input.json"
    path.write_text(json.dumps(payload, indent=2))
    log(f"input: {len(payload['changedFiles'])} changed files, {len(diff.splitlines())} diff lines")
    return path


def install_home() -> None:
    template = Path("/opt/rb/home")
    for source in template.rglob("*"):
        target = HOME / source.relative_to(template)
        if source.is_dir():
            target.mkdir(parents=True, exist_ok=True)
        elif not target.exists():
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)


# --- run ---------------------------------------------------------------------


def run_graph(input_path: Path) -> tuple[str | None, dict | None]:
    command = [
        "zeroshot", "run",
        "--title", f"Review {ENV.get('RB_NWO', '')}#{ENV.get('RB_PR_NUMBER', '')}",
        "--graph", str(CONFIG / "review.graph.json"),
        "--runtime-config", str(CONFIG / "review.runtime.json"),
        "--input", str(input_path),
        "--cwd", str(REPO),
        "--submission-key", f"review-{ENV.get('RB_HEAD', 'head')[:12]}-{ENV.get('RB_ATTEMPT', '1')}-{int(START)}",
    ]
    # The agents get the environment without the contract's RB_* variables.
    environment = {name: value for name, value in ENV.items() if not name.startswith("RB_")}
    process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True, env=environment)
    state = {"run_id": None, "terminal": None}

    def read() -> None:
        for line in process.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            state["run_id"] = state["run_id"] or event.get("runId")
            status = event.get("status") or {}
            if isinstance(status, dict) and status.get("terminalResult"):
                state["terminal"] = status["terminalResult"]

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    while process.poll() is None:
        if time.time() - START > DEADLINE:
            log(f"deadline reached; stopping run {state['run_id']}")
            if state["run_id"]:
                subprocess.run(["zeroshot", "force-stop", state["run_id"]], capture_output=True, timeout=90)
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                process.kill()
            break
        time.sleep(2)
    reader.join(timeout=10)
    log(f"zeroshot exited {process.returncode}; run {state['run_id']}")
    return state["run_id"], state["terminal"]


def completed_outputs(run_id: str | None) -> dict[str, list[dict]]:
    """Outputs of every completed node, from the run's local ledger."""
    outputs: dict[str, list[dict]] = {}
    database = STATE / "runs" / str(run_id) / "runs.sqlite3"
    if not run_id or not database.exists():
        return outputs
    try:
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        for (text,) in connection.execute("select event_json from v2_run_events order by sequence"):
            event = json.loads(text)
            if event.get("kind") != "node_completed":
                continue
            completion = event.get("completion") or {}
            outcome = completion.get("outcome") or {}
            node = (completion.get("reference") or {}).get("node")
            if node and isinstance(outcome.get("output"), dict):
                outputs.setdefault(node, []).append(outcome["output"])
    except sqlite3.Error as error:
        log(f"ledger read failed: {error}")
    return outputs


def salvage(outputs: dict[str, list[dict]]) -> tuple[str, list[dict]]:
    """Finalized findings where a unit was finalized, the reviewers' raw findings elsewhere."""
    raw = [f for node in sorted(n for n in outputs if n.startswith(("unit_review_", "hunt_")))
           for output in outputs[node] for f in output.get("findings", [])]
    finals = [f for node in ("finalize_unit", "finalize_rest") for output in outputs.get(node, [])
              for f in output.get("findings", [])]
    if finals:
        covered = {f.get("file") for f in finals}
        return "partial-final", finals + [f for f in raw if f.get("file") not in covered]
    return "raw", raw


# --- output ------------------------------------------------------------------


def line_count(path: Path) -> int | None:
    try:
        with path.open("rb") as handle:
            return sum(1 for _ in handle) or 1
    except OSError:
        return None


def to_contract(findings: list[dict]) -> list[dict]:
    out, seen = [], set()
    for finding in findings:
        file = str(finding.get("file") or "").strip()
        for prefix in (str(REPO) + "/", "./", "/"):
            if file.startswith(prefix):
                file = file[len(prefix):]
        title = str(finding.get("title") or "").strip()
        body = str(finding.get("body") or "").strip()
        if not file or not (title or body):
            continue
        try:
            start = int(finding.get("startLine") or 1)
            end = int(finding.get("endLine") or start)
        except (TypeError, ValueError):
            start = end = 1
        start, end = max(1, min(start, end)), max(1, max(start, end))
        total = line_count(REPO / file)
        if total is not None:
            start, end = min(start, total), min(end, total)
        message = f"{title}\n\n{body}" if title and body else (title or body)
        key = (file, start, end, message)
        if key in seen:
            continue
        seen.add(key)
        out.append({"file": file, "start_line": start, "end_line": end, "message": message, "producer": AGENT})
    return out


def write_findings(pr: dict, findings: list[dict]) -> None:
    document = {
        "pr": {
            "repo": pr.get("repo") or f"https://github.com/{ENV.get('RB_NWO', '')}",
            "pr_number": int(ENV.get("RB_PR_NUMBER") or pr.get("pr_number")),
            "base": ENV.get("RB_BASE", pr.get("base")),
            "head": ENV.get("RB_HEAD", pr.get("head")),
        },
        "agent": AGENT,
        "findings": findings,
    }
    temporary = OUT.with_suffix(".tmp")
    temporary.write_text(json.dumps(document, indent=2))
    json.loads(temporary.read_text())
    temporary.replace(OUT)


def remove_scratch(present_at_start: set[str]) -> None:
    """Agents sometimes leave scratch files in the output directory; only the findings file belongs there."""
    for entry in OUT.parent.iterdir():
        try:
            if entry == OUT or entry.name in present_at_start:
                continue
            shutil.rmtree(entry) if entry.is_dir() and not entry.is_symlink() else entry.unlink()
            log(f"removed scratch output {entry.name}")
        except OSError as error:
            log(f"could not remove {entry.name}: {error}")


def main() -> int:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    present_at_start = {entry.name for entry in OUT.parent.iterdir()}
    pr = json.loads(Path(ENV.get("RB_PR_JSON", "/work/pr/pr.json")).read_text())
    settings = {name: value for name, value in sorted(ENV.items()) if name.startswith("RB_CONFIG_")}
    log(f"configuration: {json.dumps(settings) if settings else 'none declared'}")
    summary = {"graph_status": None, "source": None, "completed_nodes": {}}
    findings: list[dict] = []
    try:
        install_home()
        run_id, result = run_graph(write_input(pr))
        result = result or {}
        outputs = completed_outputs(run_id)
        summary["graph_status"] = result.get("status")
        summary["completed_nodes"] = {node: len(items) for node, items in sorted(outputs.items())}
        if result.get("status") == "succeeded":
            output = result.get("output", {})
            summary["source"] = "graph"
            raw = [f for group in output.get("findings", []) for f in group] + list(output.get("restFindings", []))
        else:
            summary["source"], raw = salvage(outputs)
        findings = to_contract(raw)
    except Exception as error:  # the contract needs a file even when the run breaks
        summary["error"] = f"{type(error).__name__}: {error}"
        log(f"error: {summary['error']}")
    write_findings(pr, findings)
    remove_scratch(present_at_start)
    summary["findings"] = len(findings)
    summary["seconds"] = round(time.time() - START, 1)
    log("summary " + json.dumps(summary))
    reviewed = any(node.startswith(REVIEWERS) for node in summary["completed_nodes"])
    return 0 if reviewed else 1


if __name__ == "__main__":
    sys.exit(main())
