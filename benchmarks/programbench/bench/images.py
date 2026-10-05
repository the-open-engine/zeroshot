"""Build the proxy and agent images and manage the isolated network."""

from __future__ import annotations

import hashlib
import re
import json
import shlex
import shutil
import tarfile
import time
from pathlib import Path
from typing import Any

from . import ROOT
from .config import Experiment, pins
from .util import docker, download, log, run, sha256_file

# Claude Code attempts: tools that would reach the web from Anthropic's side, talk to other sessions,
# or schedule work beyond the node. Everything else is Claude Code's default toolset.
CLAUDE_DISALLOWED_TOOLS = ("WebSearch", "WebFetch", "SendMessage", "ListAgents", "CronCreate", "CronDelete", "CronList", "ScheduleWakeup")
# Claude Code switches set by the launcher for every launch.
CLAUDE_SWITCHES = {
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1", "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "DISABLE_AUTOUPDATER": "1", "DISABLE_ERROR_REPORTING": "1", "DISABLE_TELEMETRY": "1",
}
# Zeroshot's own root-owned PATH; the launcher puts these directories first for Claude and its tools.
ROOT_OWNED_PATH = ("/usr/local/bin", "/usr/bin", "/bin")
GATEWAY_PORT = 8889
GATEWAY_LOG = "/var/log/zsbench/gateway.jsonl"
# What Claude Code in the attempt container sends as its API key; the gateway replaces it.
CLAUDE_PLACEHOLDER_KEY = "sk-ant-zsbench-gateway-placeholder"

# Variables that Zeroshot or Codex set per process; everything else from the image ENV is mirrored.
_NOT_MIRRORED = {"HOME", "HOSTNAME"}
# Non-interactive settings the upstream mini-SWE-agent ProgramBench baseline exported, and the
# image's plain /tmp: Zeroshot points Codex's TMPDIR into its run directory, which tool commands
# should neither write into nor learn about.
_BASELINE_ENV = {"PAGER": "cat", "MANPAGER": "cat", "LESS": "-R", "PIP_PROGRESS_BAR": "off", "TQDM_DISABLE": "1", "TMPDIR": "/tmp"}


def _extract(archive: Path, member: str, dest: Path) -> None:
    with tarfile.open(archive) as tar:
        source = tar.extractfile(member)
        if source is None:
            raise RuntimeError(f"{member} missing from {archive}")
        dest.write_bytes(source.read())
    dest.chmod(0o755)


def ensure_image(reference: str) -> None:
    if docker("image", "inspect", reference, check=False).strip() in ("", "[]"):
        log(f"pulling {reference}")
        docker("pull", "--quiet", reference, timeout=3600)


def image_env(reference: str) -> dict[str, str]:
    env = json.loads(docker("image", "inspect", reference, "--format", "{{json .Config.Env}}"))
    return dict(item.split("=", 1) for item in env or [])


def codex_config(task_env: dict[str, str]) -> str:
    lines = [f"{key} = {json.dumps(value)}" for key, value in sorted(tool_environment(task_env).items())]
    template = (ROOT / "agent" / "codex-config.toml.in").read_text()
    return template.replace("@SET@", "\n".join(lines))


def tool_environment(task_env: dict[str, str]) -> dict[str, str]:
    """The task image's environment for tool commands, with the upstream baseline's settings."""
    mirrored = {k: v for k, v in task_env.items() if k not in _NOT_MIRRORED}
    for key, value in _BASELINE_ENV.items():
        mirrored.setdefault(key, value)
    return mirrored


def claude_launcher(task_env: dict[str, str], max_output_tokens: int | None = None) -> str:
    env = tool_environment(task_env)
    rest = [d for d in env.get("PATH", "").split(":") if d and d not in ROOT_OWNED_PATH]
    env["PATH"] = ":".join([*ROOT_OWNED_PATH, *dict.fromkeys(rest)])
    env.update(CLAUDE_SWITCHES)
    if max_output_tokens is not None:
        env["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] = str(max_output_tokens)
    exports = "\n".join(f"export {key}={shlex.quote(value)}" for key, value in sorted(env.items()))
    template = (ROOT / "agent" / "claude-launcher.sh.in").read_text()
    return template.replace("@EXPORTS@", exports).replace("@DISALLOWED@", ",".join(CLAUDE_DISALLOWED_TOOLS))


def build_gateway(cache: Path) -> str:
    base = pins()["gateway_base_image"]
    digest = hashlib.sha256()
    for name in ("Dockerfile", "gateway.py"):
        digest.update((ROOT / "gateway" / name).read_bytes())
    digest.update(base.encode())
    tag = f"zsbench-gateway:{digest.hexdigest()[:16]}"
    if docker("image", "inspect", tag, check=False).strip() not in ("", "[]"):
        return tag
    log(f"building {tag}")
    docker("build", "--quiet", "--build-arg", f"BASE_IMAGE={base}", "-t", tag, str(ROOT / "gateway"), timeout=1800)
    return tag


def proxy_filter(host: str) -> bytes:
    """tinyproxy's allowlist: the one model API host (proxy/filter holds OpenAI's)."""
    return f"^{re.escape(host)}$\n".encode()


def build_proxy(cache: Path, host: str = "api.openai.com") -> str:
    base = pins()["proxy_base_image"]
    files = {name: (ROOT / "proxy" / name).read_bytes() for name in ("Dockerfile", "tinyproxy.conf")}
    files["filter"] = proxy_filter(host)
    digest = hashlib.sha256()
    for data in files.values():
        digest.update(data)
    digest.update(base.encode())
    tag = f"zsbench-proxy:{digest.hexdigest()[:16]}"
    if docker("image", "inspect", tag, check=False).strip() not in ("", "[]"):
        return tag
    log(f"building {tag}")
    context = cache / "build" / tag.replace(":", "-")
    context.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        (context / name).write_bytes(data)
    docker("build", "--quiet", "--build-arg", f"BASE_IMAGE={base}", "-t", tag, str(context), timeout=1800)
    return tag


def build_agent(exp: Experiment, cache: Path) -> tuple[str, dict[str, str]]:
    """Return the agent image tag and the rendered harness config (for the manifest)."""
    if exp.harness == "claude":
        return _build_claude_agent(exp, cache)
    tag, info = _build_codex_agent(exp, cache)
    return _reaper_image(tag, info, cache) if exp.reap_orphans else (tag, info)


def _build_codex_agent(exp: Experiment, cache: Path) -> tuple[str, dict[str, str]]:
    pin = pins()
    ensure_image(exp.task_image)
    config = codex_config(image_env(exp.task_image))
    adjustments = json.dumps({"reference_path": exp.reference_path, "doc_fixes": exp.doc_fixes}, sort_keys=True, indent=2)
    digest = hashlib.sha256()
    digest.update(exp.task_image.encode())
    digest.update(config.encode())
    digest.update(adjustments.encode())
    for name in ("Dockerfile", "prepare-task.py"):
        digest.update((ROOT / "agent" / name).read_bytes())
    for tool in ("zeroshot", "codex"):
        digest.update(pin[tool]["sha256"].encode())
    tag = f"zsbench-agent:{digest.hexdigest()[:16]}"
    info = {"tag": tag, "task_image": exp.task_image, "codex_config": config, "task_adjustments": json.loads(adjustments)}
    if docker("image", "inspect", tag, check=False).strip() not in ("", "[]"):
        return tag, info
    context = cache / "agent-context" / tag.split(":", 1)[1]
    shutil.rmtree(context, ignore_errors=True)
    context.mkdir(parents=True)
    zeroshot = download(pin["zeroshot"]["url"], pin["zeroshot"]["sha256"], cache / "downloads" / Path(pin["zeroshot"]["url"]).name)
    _extract(zeroshot, pin["zeroshot"]["member"], context / "zeroshot")
    codex = download(pin["codex"]["url"], pin["codex"]["sha256"], cache / "downloads" / Path(pin["codex"]["url"]).name)
    with tarfile.open(codex) as tar:
        tar.extractall(context / "codex-package", filter="data")
    shutil.copy(ROOT / "agent" / "Dockerfile", context / "Dockerfile")
    shutil.copy(ROOT / "agent" / "prepare-task.py", context / "prepare-task.py")
    (context / "codex-config.toml").write_text(config)
    (context / "task-adjustments.json").write_text(adjustments)
    log(f"building {tag} from {exp.task_image}")
    docker("build", "--quiet", "--build-arg", f"TASK_IMAGE={exp.task_image}", "-t", tag, str(context), timeout=3600)
    return tag, info


def _reaper_image(base: str, info: dict[str, Any], cache: Path) -> tuple[str, dict[str, Any]]:
    """The agent image with `codex` run under agent/codex-reaper.c, compiled in the image."""
    digest = hashlib.sha256(base.encode())
    for name in ("reaper.Dockerfile", "codex-reaper.c"):
        digest.update((ROOT / "agent" / name).read_bytes())
    tag = f"{base}-reaper-{digest.hexdigest()[:12]}"
    info = {**info, "tag": tag, "base_agent_image": base, "codex_reaper_sha256": sha256_file(ROOT / "agent" / "codex-reaper.c")}
    if docker("image", "inspect", tag, check=False).strip() not in ("", "[]"):
        return tag, info
    context = cache / "agent-context" / tag.split(":", 1)[1]
    shutil.rmtree(context, ignore_errors=True)
    context.mkdir(parents=True)
    shutil.copy(ROOT / "agent" / "reaper.Dockerfile", context / "Dockerfile")
    shutil.copy(ROOT / "agent" / "codex-reaper.c", context / "codex-reaper.c")
    log(f"building {tag} from {base}")
    docker("build", "--quiet", "--build-arg", f"AGENT_IMAGE={base}", "-t", tag, str(context), timeout=1800)
    return tag, info


def _build_claude_agent(exp: Experiment, cache: Path) -> tuple[str, dict[str, str]]:
    pin = pins()
    ensure_image(exp.task_image)
    launcher = claude_launcher(image_env(exp.task_image), exp.max_output_tokens)
    adjustments = json.dumps({"reference_path": exp.reference_path, "doc_fixes": exp.doc_fixes}, sort_keys=True, indent=2)
    digest = hashlib.sha256()
    for part in (exp.task_image, launcher, adjustments):
        digest.update(part.encode())
    for name in ("claude.Dockerfile", "prepare-task.py"):
        digest.update((ROOT / "agent" / name).read_bytes())
    for tool in ("zeroshot", "claude_code"):
        digest.update(pin[tool]["sha256"].encode())
    tag = f"zsbench-agent-claude:{digest.hexdigest()[:16]}"
    info = {"tag": tag, "task_image": exp.task_image, "harness_config": launcher, "task_adjustments": json.loads(adjustments)}
    if docker("image", "inspect", tag, check=False).strip() not in ("", "[]"):
        return tag, info
    context = cache / "agent-context" / tag.split(":", 1)[1]
    shutil.rmtree(context, ignore_errors=True)
    context.mkdir(parents=True)
    zeroshot = download(pin["zeroshot"]["url"], pin["zeroshot"]["sha256"], cache / "downloads" / Path(pin["zeroshot"]["url"]).name)
    _extract(zeroshot, pin["zeroshot"]["member"], context / "zeroshot")
    claude = download(pin["claude_code"]["url"], pin["claude_code"]["sha256"], cache / "downloads" / Path(pin["claude_code"]["url"]).name)
    _extract(claude, pin["claude_code"]["member"], context / "claude")
    shutil.copy(ROOT / "agent" / "claude.Dockerfile", context / "Dockerfile")
    shutil.copy(ROOT / "agent" / "prepare-task.py", context / "prepare-task.py")
    (context / "claude-launcher.sh").write_text(launcher)
    (context / "task-adjustments.json").write_text(adjustments)
    log(f"building {tag} from {exp.task_image}")
    docker("build", "--quiet", "--build-arg", f"TASK_IMAGE={exp.task_image}", "-t", tag, str(context), timeout=3600)
    return tag, info


def gateway_settings(exp: Experiment, image: str) -> dict[str, str] | None:
    """Model gateway settings for Claude attempts (None for Codex, which uses the allowlist proxy)."""
    if exp.harness != "claude":
        return None
    cap = exp.limits.get("usd_cap_per_attempt")
    return {
        "image": image, "secret_env": exp.secret_env, "prices": json.dumps(exp.pricing["usd_per_million_tokens"], sort_keys=True), "cap": str(cap) if cap else "",
        "upstream": exp.provider, "routing": json.dumps(exp.provider_routing, sort_keys=True) if exp.provider_routing else "", "models": json.dumps([exp.model]),
        "text_only": "" if exp.image_input else "1",
    }


def container_env(exp: Experiment, network: "Network") -> list[str]:
    """``docker run`` environment for an agent container. Codex reaches its API through the
    allowlist proxy; Claude Code reaches the gateway directly with a placeholder key and has no
    proxy settings at all, so tool commands have no route out."""
    if exp.harness == "claude":
        return ["-e", f"ANTHROPIC_BASE_URL={network.proxy_url}", "-e", f"ANTHROPIC_API_KEY={CLAUDE_PLACEHOLDER_KEY}"]
    env = []
    for name in ("HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"):
        env += ["-e", f"{name}={network.proxy_url}"]
    for name in ("NO_PROXY", "no_proxy"):
        env += ["-e", f"{name}=localhost,127.0.0.1,::1"]
    return env


# The attempt's container reaches its proxy (or gateway) by this alias on the attempt's own network.
# Container names can be longer than a DNS label's 63 characters, and such a name does not resolve
# (sol-xhigh-ascii-image-converter-single's proxies did not).
PROXY_ALIAS = "egress-proxy"


class Network:
    """A private internal Docker network whose only exit is its own allowlist proxy.

    Every attempt gets one, so concurrent attempts cannot reach each other and each proxy log
    belongs to exactly one attempt.
    """

    def __init__(self, name: str, proxy_image: str, labels: tuple[str, ...] = (), gateway: dict[str, str] | None = None):
        """``gateway`` (Claude attempts) replaces the allowlist proxy with the model gateway: the
        image, the key variable to hand it, its price table (JSON) and optional spending cap."""
        self.name = name
        self.proxy = f"{name}-proxy"
        self.proxy_image = proxy_image
        self.gateway = gateway
        self.labels = [arg for label in ("zsbench=1", *labels) for arg in ("--label", label)]

    @property
    def proxy_url(self) -> str:
        return f"http://{PROXY_ALIAS}:{GATEWAY_PORT if self.gateway else 8888}"

    def up(self) -> None:
        self.down()
        docker("network", "create", "--internal", *self.labels, self.name)
        if self.gateway:
            self._gateway_up()
            return
        docker("run", "-d", "--name", self.proxy, *self.labels, "--restart", "unless-stopped", "--network", self.name, "--network-alias", PROXY_ALIAS, self.proxy_image)
        docker("network", "connect", "bridge", self.proxy)

    def _gateway_up(self) -> None:
        """The gateway container idles; its server starts by `docker exec` with the key named in -e
        (the value comes from this process's environment), so the key is in neither an argument
        nor the container's configuration."""
        gateway = self.gateway or {}
        docker("run", "-d", "--name", self.proxy, *self.labels, "--network", self.name, "--network-alias", PROXY_ALIAS, gateway["image"])
        docker("network", "connect", "bridge", self.proxy)
        settings = ["-e", f"ZSBENCH_PRICES={gateway['prices']}", *(["-e", f"ZSBENCH_USD_CAP={gateway['cap']}"] if gateway.get("cap") else [])]
        settings += ["-e", f"ZSBENCH_UPSTREAM={gateway.get('upstream') or 'anthropic'}", "-e", f"ZSBENCH_MODELS={gateway.get('models') or ''}"]
        if gateway.get("routing"):
            settings += ["-e", f"ZSBENCH_PROVIDER_ROUTING={gateway['routing']}"]
        if gateway.get("text_only"):
            settings += ["-e", "ZSBENCH_TEXT_ONLY=1"]
        docker("exec", "-d", "-e", gateway["secret_env"], *settings, self.proxy, "python3", "/opt/zsbench/gateway.py")
        for _ in range(60):
            if '"event": "listening"' in docker("exec", self.proxy, "cat", GATEWAY_LOG, check=False):
                return
            time.sleep(1)
        raise RuntimeError("the model gateway did not start")

    def proxy_log(self) -> str:
        if self.gateway:
            return docker("exec", self.proxy, "cat", GATEWAY_LOG, check=False)
        result = run(["docker", "logs", self.proxy], check=False)
        return result.stdout.decode(errors="replace") + result.stderr.decode(errors="replace")

    def down(self) -> None:
        docker("rm", "-f", self.proxy, check=False)
        docker("network", "rm", self.name, check=False)
