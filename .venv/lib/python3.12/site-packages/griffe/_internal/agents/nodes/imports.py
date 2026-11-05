# SPDX-License-Identifier: ISC

# Copyright (c) 2021, Timothée Mazzucotelli and contributors

# Permission to use, copy, modify, and/or distribute this software for any
# purpose with or without fee is hereby granted, provided that the above
# copyright notice and this permission notice appear in all copies.

# THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
# WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
# MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
# ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
# WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
# ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
# OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

# This module contains utilities for working with imports and relative imports.

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import ast

    from griffe._internal.models import Module


def relative_to_absolute(node: ast.ImportFrom, name: ast.alias, current_module: Module) -> str:
    """Convert a relative import path to an absolute one.

    Parameters:
        node: The "from ... import ..." AST node.
        name: The imported name.
        current_module: The module in which the import happens.

    Returns:
        The absolute import path.
    """
    level = node.level
    if (level > 0 and current_module.is_package) or current_module.is_subpackage:
        level -= 1
    while level > 0 and current_module.parent is not None:
        current_module = current_module.parent  # ty:ignore[invalid-assignment]
        level -= 1
    base = current_module.path + "." if node.level > 0 else ""
    node_module = node.module + "." if node.module else ""
    return base + node_module + name.name
