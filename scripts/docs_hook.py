"""Add release identity and generated protocol files to the built documentation site."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any

_ROOT = Path(__file__).resolve().parents[1]
_PROTOCOL_SOURCE = _ROOT / "protocol/openengine-cluster/v1"
_PROTOCOL_FILES = (
    ("compiled-ir.schema.json", "compiled-ir.schema.json"),
    ("graph.schema.json", "graph.schema.json"),
    ("native-v2-observation.schema.json", "run-observation.schema.json"),
    ("openrpc.json", "openrpc.json"),
    ("schema.json", "schema.json"),
    ("worker.schema.json", "worker.schema.json"),
)
_RELEASE_VERSION = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
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
    docs_version = os.environ.get("ZEROSHOT_DOCS_VERSION", "dev")
    if docs_version != "dev" and _RELEASE_VERSION.fullmatch(docs_version) is None:
        raise ValueError("ZEROSHOT_DOCS_VERSION must be dev or vX.Y.Z")

    source_commit = os.environ.get("ZEROSHOT_DOCS_COMMIT") or _git_commit()
    if _SOURCE_COMMIT.fullmatch(source_commit) is None:
        raise ValueError("ZEROSHOT_DOCS_COMMIT must be a full lowercase Git commit")

    python_version = os.environ.get("ZEROSHOT_PYTHON_DOCS_VERSION")
    if docs_version != "dev" and python_version is None:
        python_version = f"{docs_version.removeprefix('v')}.post1"
    return docs_version, source_commit, python_version


def _manifest() -> dict[str, Any]:
    docs_version, source_commit, python_version = _identity()
    product_version = None if docs_version == "dev" else docs_version.removeprefix("v")
    return {
        "schemaVersion": 1,
        "docsVersion": docs_version,
        "productVersion": product_version,
        "pythonSdkVersion": python_version,
        "sourceCommit": source_commit,
        "routes": {
            "overview": "",
            "install": "getting-started/install/",
            "quickstart": "getting-started/first-run/",
            "execution": "concepts/execution/",
            "runtimePlan": "concepts/runtimes-and-connections/",
            "targets": "concepts/targets/",
            "runControl": "guides/observe-and-control/",
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


def on_post_build(*, config: Any, **_: Any) -> None:
    """Copy generated protocol artifacts and write the snapshot manifest."""
    site_dir = Path(config.site_dir)
    destination = site_dir / "reference/cluster"
    destination.mkdir(parents=True, exist_ok=True)
    for source_name, public_name in _PROTOCOL_FILES:
        shutil.copyfile(_PROTOCOL_SOURCE / source_name, destination / public_name)

    manifest = json.dumps(_manifest(), indent=2, sort_keys=True)
    (site_dir / "manifest.json").write_text(f"{manifest}\n", encoding="utf-8")
