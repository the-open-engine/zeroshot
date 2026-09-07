"""Release and CI topology checks owned by the Python SDK lane."""

from __future__ import annotations

import json
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[3]


def test_sdk_release_is_manual_and_zeroshot_release_triggered() -> None:
    package = (_ROOT / "sdks/python/pyproject.toml").read_text(encoding="utf-8")
    workflow = (_ROOT / ".github/workflows/release-python.yml").read_text(encoding="utf-8")
    zeroshot_workflow = (_ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    assert "workflow_dispatch:" in workflow
    assert "workflow_call:" in workflow
    assert 'name = "the-open-engine-zeroshot"' in package
    assert "glob(f'the_open_engine_zeroshot-{version}-py3-none-*.whl')" in workflow
    assert 'zeroshot_tag="v$ZEROSHOT_VERSION"' in workflow
    assert 'sdk_tag="zeroshot-python-v${ZEROSHOT_VERSION}_${SDK_REVISION}"' in workflow
    assert 'package_version="${ZEROSHOT_VERSION}.post${SDK_REVISION}"' in workflow
    release_targets = json.loads(
        (_ROOT / "distribution/zeroshot-targets.json").read_text(encoding="utf-8")
    )
    targets = {entry["target"] for entry in release_targets}
    assert workflow.count("wheel-platform:") == len(targets)
    assert all(f"- target: {target}" in workflow for target in targets)
    assert "--pattern SHA256SUMS" in workflow
    assert "checksum mismatch for {archive}" in workflow
    assert '[[ "$reported_version" == "zeroshot $ZEROSHOT_VERSION" ]]' in workflow
    assert "pypa/gh-action-pypi-publish@" in workflow
    assert "python-sdk-release:" in zeroshot_workflow
    assert "sdk_revision: 1" in zeroshot_workflow
    assert "uses: ./.github/workflows/release-python.yml" in zeroshot_workflow


def test_python_ci_is_selected_without_native_for_sdk_only_changes() -> None:
    classifier = (_ROOT / ".github/ci-path-classifier.js").read_text(encoding="utf-8")
    workflow = (_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    assert "'sdks/python/'" in classifier
    assert "selected.has('python')" in classifier
    assert "const native = selected.has('native')" in classifier
    assert "python-check:" in workflow
    assert "needs.classify.outputs.python == 'true'" in workflow
