"""mkdocs-autorefs package.

Automatically link across pages in MkDocs.
"""

from __future__ import annotations

from mkdocs_autorefs._internal.backlinks import Backlink, BacklinkCrumb, BacklinksTreeProcessor
from mkdocs_autorefs._internal.plugin import AutorefsConfig, AutorefsPlugin
from mkdocs_autorefs._internal.references import (
    AUTO_REF_RE,
    AUTOREF_RE,
    AnchorScannerTreeProcessor,
    AutorefsExtension,
    AutorefsHookInterface,
    AutorefsInlineProcessor,
    HeadingScannerTreeProcessor,
    fix_ref,
    fix_refs,
    relative_url,
)

__all__: list[str] = [
    "AUTOREF_RE",
    "AUTO_REF_RE",
    "AnchorScannerTreeProcessor",
    "AutorefsConfig",
    "AutorefsExtension",
    "AutorefsHookInterface",
    "AutorefsInlineProcessor",
    "AutorefsPlugin",
    "Backlink",
    "BacklinkCrumb",
    "BacklinksTreeProcessor",
    "HeadingScannerTreeProcessor",
    "fix_ref",
    "fix_refs",
    "relative_url",
]
