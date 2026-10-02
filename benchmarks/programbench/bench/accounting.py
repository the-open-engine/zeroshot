"""Token usage and cost, priced from the harness's own session transcripts (Codex or Claude Code).

Zeroshot's ledger cannot be summed directly: when a node resumes the same Codex thread (the
builder uses ``sessionScope: node_instance``), Codex reports the thread's running total and the
ledger stores it as that execution's usage, so later builder rounds would be counted again. Each
Codex transcript (``rollout-*.jsonl``) is one thread; its last ``token_count`` event carries the
thread's true cumulative usage. Every graph execution of a node starts with the node's prompt
("Authored instructions"), so the totals at those prompts split a thread into rounds. One round
can span several Codex turns (Zeroshot continues a turn after a provider error, or asks for a
corrected response), which is why turns are not used.

Claude Code transcripts (``.claude/projects/*/<session>.jsonl``) log usage per API response instead,
repeated on every content block of that response, so responses are counted once by message id.
Anthropic's ``input_tokens`` excludes cache reads and writes; totals here include them, as for Codex.
A subagent's transcript joins its parent session, in the round that was running when it ran.
"""

from __future__ import annotations

import json
import tarfile
from collections import defaultdict
from collections.abc import Iterable
from pathlib import Path
from typing import Any

from . import ledger
from .config import prompt

FIELDS = ("inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens")
_CODEX = {"input_tokens": "inputTokens", "output_tokens": "outputTokens", "cached_input_tokens": "cacheReadInputTokens", "cache_write_input_tokens": "cacheCreationInputTokens"}
# Claude only: the part of cacheCreationInputTokens written to the 1-hour cache (priced higher).
ONE_HOUR = "cacheCreation1hInputTokens"


def _zero() -> dict[str, int]:
    return dict.fromkeys(FIELDS, 0)


def _from_codex(usage: dict[str, Any]) -> dict[str, int]:
    return {ours: int(usage.get(theirs) or 0) for theirs, ours in _CODEX.items()}


def _minus(a: dict[str, int], b: dict[str, int]) -> dict[str, int]:
    return {k: a[k] - b[k] for k in FIELDS}


def _add(a: dict[str, int], b: dict[str, int]) -> None:
    for k in FIELDS:
        a[k] += b[k]
    if b.get(ONE_HOUR):
        a[ONE_HOUR] = a.get(ONE_HOUR, 0) + b[ONE_HOUR]


def _from_claude(usage: dict[str, Any]) -> dict[str, int]:
    read = int(usage.get("cache_read_input_tokens") or 0)
    write = int(usage.get("cache_creation_input_tokens") or 0)
    tokens = {"inputTokens": int(usage.get("input_tokens") or 0) + read + write, "outputTokens": int(usage.get("output_tokens") or 0), "cacheReadInputTokens": read, "cacheCreationInputTokens": write}
    one_hour = int((usage.get("cache_creation") or {}).get("ephemeral_1h_input_tokens") or 0)
    if one_hour:
        tokens[ONE_HOUR] = one_hour
    return tokens


def claude_text(message: dict[str, Any] | None) -> str:
    """The typed text of a Claude Code user message (not tool results)."""
    content = (message or {}).get("content")
    if isinstance(content, str):
        return content
    return " ".join(str(c.get("text", "")) for c in content or [] if isinstance(c, dict) and c.get("type") == "text")


def node_of_session(first_prompt: str) -> str:
    if prompt("checker") in first_prompt:
        return "check"
    if prompt("builder") in first_prompt:
        return "build"
    return "other"


def is_node_prompt(record: dict[str, Any]) -> bool:
    """The user message that starts one graph execution of a node (Codex or Claude Code record).
    Claude Code's compaction summaries can quote the instructions, but they start no execution."""
    if record.get("type") == "user" and "message" in record:
        return not record.get("isMeta") and not record.get("isCompactSummary") and "Authored instructions" in claude_text(record.get("message"))
    payload = record.get("payload") or {}
    if payload.get("type") != "message" or payload.get("role") != "user":
        return False
    return any("Authored instructions" in str(c.get("text", "")) for c in payload.get("content") or [] if isinstance(c, dict))


def sessions(trajectories: Path) -> list[dict[str, Any]]:
    """One record per Codex thread: its node, cumulative usage, and usage per round."""
    out = []
    if not trajectories.exists():
        return out
    with tarfile.open(trajectories) as tar:
        for member in tar:
            if not (member.isfile() and "/sessions/" in member.name and member.name.endswith(".jsonl")):
                continue
            data = tar.extractfile(member)
            if data is None:
                continue
            first_prompt, total, rounds, round_start, decreased = "", _zero(), [], None, False
            for line in data.read().splitlines():
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    continue
                payload = record.get("payload") or {}
                if is_node_prompt(record):
                    if not first_prompt:
                        first_prompt = " ".join(str(c.get("text", "")) for c in payload.get("content") or [] if isinstance(c, dict))
                    if round_start is not None:
                        rounds.append(_minus(total, round_start))
                    round_start = dict(total)
                elif payload.get("type") == "token_count" and (payload.get("info") or {}).get("total_token_usage"):
                    new_total = _from_codex(payload["info"]["total_token_usage"])
                    decreased |= any(new_total[k] < total[k] for k in FIELDS)  # cumulative usage must not shrink
                    total = new_total
            if round_start is not None:
                rounds.append(_minus(total, round_start))
            elif total != _zero():  # usage without any node prompt
                rounds.append(dict(total))
            out.append({"file": member.name, "node": node_of_session(first_prompt), "total": total, "rounds": rounds, "usage_decreased": decreased})
    return out


def claude_transcripts(trajectories: Path) -> dict[str, list[dict[str, Any]]]:
    """Claude Code transcripts in an archive, by member name (sessions and subagents)."""
    files: dict[str, list[dict[str, Any]]] = {}
    if not trajectories.exists():
        return files
    with tarfile.open(trajectories) as tar:
        for member in tar:
            name = member.name.removeprefix("./")
            if not (member.isfile() and name.startswith(".claude/projects/") and name.endswith(".jsonl")):
                continue
            data = tar.extractfile(member)
            records = []
            for line in (data.read() if data else b"").splitlines():
                try:
                    records.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
            files[name] = records
    return files


def claude_parent(name: str) -> str | None:
    """The session id a subagent transcript belongs to (``<session>/subagents/agent-*.jsonl``)."""
    parts = name.split("/")
    return parts[parts.index("subagents") - 1] if "subagents" in parts[:-1] else None


def _claude_messages(records: list[dict[str, Any]]) -> dict[str, tuple[str, int, dict[str, Any]]]:
    """Each API response once: message id -> (first timestamp, node prompts before it, usage)."""
    messages: dict[str, tuple[str, int, dict[str, Any]]] = {}
    prompts = 0
    for record in records:
        if is_node_prompt(record):
            prompts += 1
        elif record.get("type") == "assistant":
            message = record.get("message") or {}
            key = message.get("id") or record.get("requestId") or record.get("uuid")
            if key and message.get("usage"):
                first = messages.get(key)
                messages[key] = (first[0] if first else str(record.get("timestamp") or ""), first[1] if first else prompts, message["usage"])
    return messages


def claude_sessions(trajectories: Path) -> list[dict[str, Any]]:
    """One record per Claude Code session with usage per round (subagents included), in order."""
    files = claude_transcripts(trajectories)
    out: dict[str, dict[str, Any]] = {}
    prompt_times: dict[str, list[str]] = {}
    for name, records in files.items():
        if claude_parent(name) is not None:
            continue
        first_prompt = next((claude_text(r.get("message")) for r in records if is_node_prompt(r)), "")
        prompt_times[Path(name).stem] = [str(r.get("timestamp") or "") for r in records if is_node_prompt(r)]
        rounds = [_zero() for _ in range(max(1, len(prompt_times[Path(name).stem])))]
        for _, before, usage in _claude_messages(records).values():
            _add(rounds[max(0, before - 1)], _from_claude(usage))
        started = min((str(r.get("timestamp")) for r in records if r.get("timestamp")), default="")
        out[Path(name).stem] = {"file": name, "node": node_of_session(first_prompt), "rounds": rounds, "usage_decreased": False, "started": started}
    for name, records in files.items():
        parent = claude_parent(name)
        if parent is None:
            continue
        session = out.get(parent)
        if session is None:  # a subagent of an unknown session: account it on its own
            session = out.setdefault(parent, {"file": name, "node": "other", "rounds": [_zero()], "usage_decreased": False, "started": ""})
        times = prompt_times.get(parent, [])
        for timestamp, _, usage in _claude_messages(records).values():
            index = sum(1 for t in times if t and t <= timestamp) - 1
            _add(session["rounds"][min(max(0, index), len(session["rounds"]) - 1)], _from_claude(usage))
    for session in out.values():
        total = _zero()
        for round_usage in session["rounds"]:
            _add(total, round_usage)
        session["total"] = total
    return sorted(out.values(), key=lambda s: (s["started"], s["file"]))


def usage(attempt_dir: Path) -> dict[str, Any]:
    """Per-node and total usage from transcripts, the builder's first turn separately, and the
    ledger's figure for comparison."""
    per_node: dict[str, dict[str, int]] = defaultdict(_zero)
    first_build_round = None
    # Transcript names start with their creation time, so the earliest builder thread holds build
    # 1 (a build error makes Zeroshot start a new thread for the next round).
    records = sorted(sessions(attempt_dir / "trajectories.tar.gz"), key=lambda s: Path(s["file"]).name)
    claude = claude_sessions(attempt_dir / "trajectories.tar.gz")
    records += claude
    for session in records:
        _add(per_node[session["node"]], session["total"])
        _add(per_node["total"], session["total"])
        if session["node"] == "build" and session["rounds"] and first_build_round is None:
            first_build_round = dict(session["rounds"][0])
    ledger_events = ledger.events(attempt_dir / "trajectories.tar.gz")
    ledger_nodes = usage_from_events(ledger_events, anthropic=bool(claude)) if ledger_events else {}
    return {
        "nodes": dict(per_node),
        "first_build_round": first_build_round,
        "sessions": [{"node": s["node"], "rounds": len(s["rounds"]), "total": s["total"], "usage_decreased": s["usage_decreased"]} for s in records],
        "ledger_total": ledger_nodes.get("total"),
        "ledger_nodes": ledger_nodes,
    }


def usage_from_events(events: Iterable[dict[str, Any]], anthropic: bool = False) -> dict[str, dict[str, int]]:
    """Zeroshot ledger view (kept as a cross-check; over-counts resumed builder rounds). Zeroshot
    copies Claude's usage unnormalized, so with ``anthropic`` input tokens gain the cache tokens."""
    node_of_execution: dict[int, str] = {}
    totals: dict[str, dict[str, int]] = defaultdict(_zero)
    for event in events:
        if event.get("kind") == "node_started":
            reference = event.get("reference") or {}
            node_of_execution[int(reference.get("execution", -1))] = reference.get("node", "?")
        elif event.get("kind") == "token_usage_observed":
            node = node_of_execution.get(int(event.get("execution", -1)), "?")
            observed = event.get("usage") or {}
            for field in FIELDS:
                value = int(observed.get(field) or 0)
                if anthropic and field == "inputTokens":
                    value += int(observed.get("cacheReadInputTokens") or 0) + int(observed.get("cacheCreationInputTokens") or 0)
                totals[node][field] += value
                totals["total"][field] += value
    return {node: dict(values) for node, values in totals.items()}


def cost(tokens: dict[str, int], pricing: dict[str, Any]) -> float:
    price = pricing["usd_per_million_tokens"]
    cache_read = tokens.get("cacheReadInputTokens", 0)
    cache_write = tokens.get("cacheCreationInputTokens", 0)
    uncached = max(0, tokens.get("inputTokens", 0) - cache_read - cache_write)
    one_hour = tokens.get(ONE_HOUR, 0)
    return (
        uncached * price["input"]
        + cache_read * price["cached_input"]
        + (cache_write - one_hour) * price["cache_write"]
        + one_hour * price.get("cache_write_1h", price["cache_write"])
        + tokens.get("outputTokens", 0) * price["output"]
    ) / 1_000_000
