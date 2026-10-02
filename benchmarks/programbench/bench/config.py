"""Experiment configuration."""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import ROOT

ARMS = ("loop", "single")
EFFORTS = ("low", "medium", "high", "xhigh", "max")
# Agent harness -> model provider it runs against, and the API key variable that provider needs.
HARNESSES = {"codex": "openai", "claude": "anthropic"}  # each harness's default provider
# Codex talks to its provider through the allowlist proxy; Claude Code always talks Anthropic's API to
# the model gateway, which forwards to the provider named here.
PROVIDERS = {"codex": ("openai", "openrouter"), "claude": ("anthropic", "openrouter")}
SECRET_ENVS = {"openai": "OPENAI_API_KEY", "anthropic": "ANTHROPIC_API_KEY", "openrouter": "OPENROUTER_API_KEY"}
# The one host each provider's agents may reach (the egress proxy's allowlist, or the gateway's upstream).
API_HOSTS = {"openai": "api.openai.com", "anthropic": "api.anthropic.com", "openrouter": "openrouter.ai"}
# The largest output limit per response Claude Code sends, whatever CLAUDE_CODE_MAX_OUTPUT_TOKENS asks for.
CLAUDE_MAX_OUTPUT_TOKENS = 128_000
# Where ProgramBench task images put the reference executable. An experiment may move it out of the
# workspace (task.reference_path), where the agent can run it but not overwrite, move or delete it.
UPSTREAM_REFERENCE = "/workspace/executable"
# The task statement's mentions of the reference, rewritten when it moves. The build target
# (`compile.sh` producing `./executable` in the workspace root) is not one of them.
REFERENCE_MENTIONS = (
    ("The reference `./executable` is for", "The reference `{ref}` is for"),
    ("(`cp ./executable ./executable`)", "(`cp {ref} ./executable`)"),
    ("All information about the reference `./executable` must", "All information about the reference `{ref}` must"),
    ("You MUST NOT decompile `./executable` or use disassemblers", "You MUST NOT decompile `{ref}` or use disassemblers"),
    ("tracing/instrumentation tools on `./executable`", "tracing/instrumentation tools on `{ref}`"),
    ("applies ONLY to the reference `./executable`.", "applies ONLY to the reference `{ref}`."),
    ("The executable is located at `./executable` in the workspace root.", "The executable is located at `{ref}`."),
    ("delegate to the reference `./executable`", "delegate to the reference `{ref}`"),
    ("Do NOT decompile the reference `./executable`", "Do NOT decompile the reference `{ref}`"),
    ("you MUST NOT decompile `./executable` or perform", "you MUST NOT decompile `{ref}` or perform"),
)


@dataclass(frozen=True)
class AttemptSpec:
    index: int
    arm: str

    @property
    def label(self) -> str:
        return f"{self.index:02d}-{self.arm}"


@dataclass(frozen=True)
class Experiment:
    path: Path
    raw: dict[str, Any]

    @property
    def id(self) -> str:
        return self.raw["id"]

    @property
    def instance_id(self) -> str:
        return self.raw["task"]["instance_id"]

    @property
    def task_image(self) -> str:
        return self.raw["task"]["image"]

    @property
    def reference_path(self) -> str:
        return self.raw["task"].get("reference_path", UPSTREAM_REFERENCE)

    @property
    def doc_fixes(self) -> list[dict[str, str]]:
        """Exact-text corrections to the task's bundled documentation (``file``, ``old``, ``new``)."""
        return self.raw["task"].get("doc_fixes", [])

    @property
    def fidelity_reference(self) -> dict[str, str]:
        """The published submission the smoke test re-scores on this task (pins.json pins svgbob's)."""
        return self.raw["task"].get("fidelity_reference") or pins()["fidelity_reference"]

    @property
    def model(self) -> str:
        return self.raw["model"]["id"]

    @property
    def effort(self) -> str:
        return self.raw["model"]["effort"]

    @property
    def harness(self) -> str:
        return self.raw["model"].get("harness", "codex")

    @property
    def provider(self) -> str:
        return self.raw["model"].get("provider", HARNESSES[self.harness])

    @property
    def runtime_provider(self) -> str:
        """The provider Zeroshot is told about: Claude Code always sees an Anthropic-compatible API
        (the gateway), whatever the gateway's upstream is."""
        return "anthropic" if self.harness == "claude" else self.provider

    @property
    def provider_routing(self) -> dict[str, Any] | None:
        """OpenRouter provider preferences the gateway adds to every inference request."""
        return self.raw["model"].get("provider_routing")

    @property
    def max_output_tokens(self) -> int | None:
        """Claude Code's output limit per response (CLAUDE_CODE_MAX_OUTPUT_TOKENS); None keeps its
        default, which depends on whether its model catalog knows the model."""
        return self.raw["model"].get("max_output_tokens")

    @property
    def image_input(self) -> bool:
        """Whether the model accepts images; for a text-only model the gateway replaces images in
        requests with a note (Claude Code's Read tool returns image files as images)."""
        return self.raw["model"].get("image_input", True)

    @property
    def api_host(self) -> str:
        return API_HOSTS[self.provider]

    @property
    def secret_env(self) -> str:
        return SECRET_ENVS[self.provider]

    @property
    def limits(self) -> dict[str, int]:
        return self.raw["limits"]

    @property
    def resources(self) -> dict[str, Any]:
        return self.raw["resources"]

    @property
    def max_iterations(self) -> int:
        return self.raw["arms"]["loop"]["max_iterations"]

    @property
    def pricing(self) -> dict[str, Any]:
        return self.raw["pricing"]

    def attempts(self) -> list[AttemptSpec]:
        return [AttemptSpec(index + 1, arm) for index, arm in enumerate(self.raw["order"])]

    def digest(self) -> str:
        """Hash of the experiment config and every file of the benchmark that shapes a run."""
        h = hashlib.sha256()
        h.update(json.dumps(self.raw, sort_keys=True).encode())
        for path in code_files():
            h.update(str(path.relative_to(ROOT)).encode())
            h.update(path.read_bytes())
        return h.hexdigest()


def code_digest() -> str:
    """Hash of every benchmark file that shapes a run or its scoring (without an experiment)."""
    h = hashlib.sha256()
    for path in code_files():
        h.update(str(path.relative_to(ROOT)).encode())
        h.update(path.read_bytes())
    return h.hexdigest()


def code_files() -> list[Path]:
    """Files that shape a run: everything in the benchmark except results, tests and hidden files."""
    skip = {"results", "tests", "__pycache__"}
    return sorted(
        p for p in ROOT.rglob("*")
        if p.is_file() and not skip & set(p.relative_to(ROOT).parts) and not any(part.startswith(".") for part in p.relative_to(ROOT).parts) and p.suffix != ".pyc"
    )


def load(path: str | Path) -> Experiment:
    path = Path(path)
    if not path.is_absolute() and not path.exists():
        path = ROOT / path
    raw = json.loads(path.read_text())
    _validate(raw)
    return Experiment(path.resolve(), raw)


def _validate(raw: dict[str, Any]) -> None:
    if not re.fullmatch(r"[a-z0-9][a-z0-9.-]{0,62}", raw["id"]):
        raise ValueError("experiment id must be lowercase letters, digits, dots and dashes")
    if "@sha256:" not in raw["task"]["image"]:
        raise ValueError("task image must be pinned by digest")
    reference = raw["task"].get("reference_path", UPSTREAM_REFERENCE)
    if not reference.startswith("/") or (reference != UPSTREAM_REFERENCE and (reference + "/").startswith("/workspace/")):
        raise ValueError("task.reference_path must be absolute and, when moved, outside /workspace")
    for fix in raw["task"].get("doc_fixes", []):
        if set(fix) != {"file", "old", "new"} or fix["file"].startswith("/") or ".." in Path(fix["file"]).parts or not fix["old"] or fix["old"] == fix["new"]:
            raise ValueError(f"invalid doc fix: {fix}")
    fidelity = raw["task"].get("fidelity_reference")
    if fidelity is not None and (
        set(fidelity) != {"submission", "commit", "archive_url", "archive_sha256", "registry_commit"}
        or not fidelity["archive_url"].endswith(f"/{raw['task']['instance_id']}/submission.tar.gz")
        or not re.fullmatch(r"[0-9a-f]{64}", fidelity["archive_sha256"])
    ):
        raise ValueError("task.fidelity_reference must pin a published submission archive of this task by sha256")
    if raw["model"]["effort"] not in EFFORTS:
        raise ValueError(f"effort must be one of {EFFORTS}")
    harness = raw["model"].get("harness", "codex")
    if harness not in HARNESSES or raw["model"].get("provider", HARNESSES.get(harness)) not in PROVIDERS[harness]:
        raise ValueError(f"model.harness/provider must be one of {PROVIDERS}")
    routing = raw["model"].get("provider_routing")
    if routing is not None and (harness != "claude" or raw["model"].get("provider") != "openrouter" or not isinstance(routing, dict) or not routing.get("order")):
        raise ValueError("model.provider_routing pins OpenRouter providers through the Claude gateway and needs an order")
    max_output = raw["model"].get("max_output_tokens")
    if max_output is not None and (harness != "claude" or isinstance(max_output, bool) or not isinstance(max_output, int) or not 1 <= max_output <= CLAUDE_MAX_OUTPUT_TOKENS):
        raise ValueError(f"model.max_output_tokens sets Claude Code's output limit per response: an integer from 1 to {CLAUDE_MAX_OUTPUT_TOKENS}")
    image_input = raw["model"].get("image_input", True)
    if not isinstance(image_input, bool) or (not image_input and harness != "claude"):
        raise ValueError("model.image_input is true or false, and only the Claude gateway can take images out of requests")
    if "usd_cap_per_attempt" in raw["limits"] and (harness != "claude" or float(raw["limits"]["usd_cap_per_attempt"]) <= 0):
        raise ValueError("limits.usd_cap_per_attempt must be positive and needs the Claude gateway")
    arms = raw["arms"]
    if set(arms) - set(ARMS):
        raise ValueError(f"unknown arms: {set(arms) - set(ARMS)}")
    order = raw["order"]
    for arm in ARMS:
        if arm in arms and order.count(arm) != arms[arm]["repeats"]:
            raise ValueError(f"order lists {order.count(arm)} {arm} runs but repeats is {arms[arm]['repeats']}")
    if set(order) - set(arms):
        raise ValueError("order names an arm that is not configured")
    limits = raw["limits"]
    for key in ("attempt_seconds", "build_timeout_ms", "check_timeout_ms"):
        if int(limits[key]) <= 0:
            raise ValueError(f"{key} must be positive")
    if "loop" in arms and not 1 <= arms["loop"]["max_iterations"] <= 100:
        raise ValueError("max_iterations must be between 1 and 100")
    schedule = raw["eval"].get("rounds")
    if schedule is not None and (not schedule or schedule[0] != 1 or schedule != sorted(set(schedule)) or not all(isinstance(r, int) and r >= 1 for r in schedule)):
        raise ValueError("eval.rounds must be an ascending list of distinct round numbers starting at 1 (build 1 is the H1 baseline)")
    prices = raw["pricing"]["usd_per_million_tokens"]
    for key in ("input", "cached_input", "cache_write", "output", *(("cache_write_1h",) if "cache_write_1h" in prices else ())):
        if float(prices[key]) < 0:
            raise ValueError("prices must be non-negative")


def pins() -> dict[str, Any]:
    return json.loads((ROOT / "pins.json").read_text())


def prompt(name: str) -> str:
    return (ROOT / "prompts" / f"{name}.md").read_text().strip()


def task_statement(exp: Experiment) -> str:
    """The task statement, pointing at the reference wherever the experiment puts it."""
    text = prompt("task")
    if exp.reference_path == UPSTREAM_REFERENCE:
        return text
    for old, new in REFERENCE_MENTIONS:
        if text.count(old) != 1:
            raise ValueError(f"task statement does not contain {old!r} exactly once")
        text = text.replace(old, new.format(ref=exp.reference_path))
    return text
