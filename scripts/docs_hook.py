"""Add release identity and generated protocol files to the built documentation site."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from datetime import date
from pathlib import Path
from typing import Any

_ROOT = Path(__file__).resolve().parents[1]
_PROTOCOL_SOURCE = _ROOT / "protocol/openengine-cluster/v1"
_DOCS = _ROOT / "docs"
_PYTHON_SOURCE = _ROOT / "sdks/python/src"
_SNIPPET = re.compile(r'--8<-- "([^"]+)"')
_API_DIRECTIVE = re.compile(r"^::: ", re.MULTILINE)
_PROTOCOL_FILES = (
    ("compiled-ir.schema.json", "compiled-ir.schema.json"),
    ("graph.schema.json", "graph.schema.json"),
    ("native-v2-observation.schema.json", "run-observation.schema.json"),
    ("openrpc.json", "openrpc.json"),
    ("schema.json", "schema.json"),
    ("worker.schema.json", "worker.schema.json"),
)
_RELEASE_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
_SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")


def _git_commit() -> str:
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


def _identity() -> tuple[str, str, str | None]:
    docs_version = os.environ.get("ZEROSHOT_DOCS_VERSION", "current")
    product_version = os.environ.get("ZEROSHOT_PRODUCT_DOCS_VERSION") or None
    if docs_version == "current":
        if product_version is not None:
            raise ValueError(
                "Current documentation must not claim a released product version"
            )
    elif (
        product_version is None
        or _RELEASE_VERSION.fullmatch(product_version) is None
        or docs_version != "v" + product_version.rsplit(".", 1)[0]
    ):
        raise ValueError(
            "Minor documentation must name its exact X.Y.Z product version"
        )

    source_commit = os.environ.get("ZEROSHOT_DOCS_COMMIT") or _git_commit()
    if _SOURCE_COMMIT.fullmatch(source_commit) is None:
        raise ValueError("ZEROSHOT_DOCS_COMMIT must be a full lowercase Git commit")

    return docs_version, source_commit, product_version


def _route_target(route: str) -> str:
    return route + "index.html" if not route or route.endswith("/") else route


def _documented_routes(routes: dict[str, str], site_dir: Path) -> dict[str, str]:
    """Keep only routes whose page exists in this build, so older sources omit newer pages."""
    return {
        name: route
        for name, route in routes.items()
        if (site_dir / _route_target(route)).is_file()
    }


def _manifest(site_dir: Path | None = None) -> dict[str, Any]:
    docs_version, source_commit, product_version = _identity()
    publisher_commit = os.environ.get("ZEROSHOT_DOCS_PUBLISHER_COMMIT") or _git_commit()
    if _SOURCE_COMMIT.fullmatch(publisher_commit) is None:
        raise ValueError(
            "ZEROSHOT_DOCS_PUBLISHER_COMMIT must be a full lowercase Git commit"
        )
    manifest = {
        "schemaVersion": 2,
        "docsVersion": docs_version,
        "productVersion": product_version,
        "pythonSdkVersion": f"{product_version}.post1" if product_version else None,
        "sourceCommit": source_commit,
        "publisherCommit": publisher_commit,
        "routes": {
            "overview": "",
            "install": "getting-started/install/",
            "quickstart": "getting-started/first-run/",
            "execution": "concepts/execution/",
            "runtimePlan": "concepts/runtimes-and-connections/",
            "runtimePlanReference": "reference/runtime-plan/",
            "targets": "concepts/targets/",
            "runControl": "guides/observe-and-control/",
            "reviewLoop": "guides/review-loop/",
            "pythonSdk": "guides/python-sdk/",
            "cli": "zeroshot-cli/",
            "pythonApi": "reference/python/",
            "clusterApi": "reference/cluster/api/",
            "graphSpec": "reference/cluster/graph/",
            "openrpc": "reference/cluster/openrpc.json",
            "portableBindings": "reference/cluster/portable-bindings/",
            "schema": "reference/cluster/schema.json",
        },
    }
    if site_dir is not None:
        manifest["routes"] = _documented_routes(manifest["routes"], site_dir)
    return manifest


def _git(root: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments], cwd=root, check=True, capture_output=True, text=True
    )
    return completed.stdout.strip()


def _last_change(paths: list[Path], root: Path = _ROOT) -> date | None:
    """Return the day these files last changed in Git, or nothing when history cannot tell."""
    names = [str(path) for path in paths]
    try:
        # A shallow clone credits every file to its one visible commit.
        if _git(root, "rev-parse", "--is-shallow-repository") != "false":
            return None
        # Content that differs from its commit, such as a publisher's copy, has no date.
        if _git(root, "status", "--porcelain", "--", *names):
            return None
        return date.fromisoformat(_git(root, "log", "-1", "--format=%cs", "--", *names))
    except (OSError, subprocess.CalledProcessError, ValueError):
        return None


def _content_sources(source: Path, markdown: str) -> list[Path]:
    """List every file a page shows: itself, its included snippets, and the SDK for API pages."""
    sources = [source]
    sources += [_DOCS / name for name in _SNIPPET.findall(markdown)]
    if _API_DIRECTIVE.search(markdown):
        sources.append(_PYTHON_SOURCE)
    return sources


def on_config(config: Any, **_: Any) -> Any:
    """Apply the current navigation policy even when rebuilding an older release."""
    config.extra["version"] = {"provider": "mike", "default": "current", "alias": False}
    return config


def on_page_markdown(markdown: str, *, page: Any, **_: Any) -> str:
    """Show when each page last changed, under its content."""
    # Generated references cannot carry hand-authored Markdown front matter.
    descriptions = {
        "zeroshot-cli.md": (
            "zeroshot CLI reference for commands, options, graph templates, targets, "
            "connections, profiles and delivery controls."
        ),
        "reference/cluster/api.md": (
            "Open Engine Cluster Protocol API reference with method parameters, "
            "results, transport capabilities and machine-readable schemas."
        ),
    }
    if page.file.src_uri in descriptions:
        page.meta["description"] = descriptions[page.file.src_uri]
    changed = _last_change(_content_sources(Path(page.file.abs_src_path), markdown))
    if changed is not None:
        page.meta["revision_date"] = (
            f'Last updated <time datetime="{changed.isoformat()}">'
            f"{changed.day} {changed:%B %Y}</time>"
        )
    return markdown


def on_post_build(*, config: Any, **_: Any) -> None:
    """Copy generated protocol artifacts and write the snapshot manifest."""
    site_dir = Path(config.site_dir)
    destination = site_dir / "reference/cluster"
    destination.mkdir(parents=True, exist_ok=True)
    for source_name, public_name in _PROTOCOL_FILES:
        shutil.copyfile(_PROTOCOL_SOURCE / source_name, destination / public_name)

    manifest = json.dumps(_manifest(site_dir), indent=2, sort_keys=True)
    (site_dir / "manifest.json").write_text(f"{manifest}\n", encoding="utf-8")
