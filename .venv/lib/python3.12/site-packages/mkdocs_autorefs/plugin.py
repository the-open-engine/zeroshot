"""Deprecated. Import from 'mkdocs_autorefs' instead."""

# YORE: Bump 2: Remove file.

import warnings
from typing import Any

from mkdocs_autorefs._internal import plugin


def __getattr__(name: str) -> Any:
    warnings.warn(
        "Importing from 'mkdocs_autorefs.plugin' is deprecated. Import directly from 'mkdocs_autorefs' instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    return getattr(plugin, name)
