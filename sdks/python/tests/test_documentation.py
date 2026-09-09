from __future__ import annotations

import re
from pathlib import Path

import pytest

REPOSITORY_ROOT = Path(__file__).parents[3]
PYTHON_FENCE = re.compile(r"^```python\n(.*?)^```$", re.MULTILINE | re.DOTALL)
DOCUMENTS = (
    REPOSITORY_ROOT / "docs" / "guides" / "python-sdk.md",
    REPOSITORY_ROOT / "sdks" / "python" / "README.md",
)


@pytest.mark.parametrize("document", DOCUMENTS, ids=lambda path: path.name)
def test_documented_python_snippets_compile(document: Path) -> None:
    snippets = PYTHON_FENCE.findall(document.read_text(encoding="utf-8"))

    assert snippets
    for number, snippet in enumerate(snippets, start=1):
        compile(snippet, f"{document}:python block {number}", "exec")
