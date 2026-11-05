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

# This module contains utilities for docstrings parsers.

from __future__ import annotations

from ast import Expression, PyCF_ONLY_AST
from contextlib import suppress
from functools import lru_cache
from typing import TYPE_CHECKING, cast

from griffe._internal.enumerations import LogLevel
from griffe._internal.exceptions import BuiltinModuleError
from griffe._internal.expressions import safe_get_annotation
from griffe._internal.logger import logger

if TYPE_CHECKING:
    from griffe._internal.expressions import Expr
    from griffe._internal.models import Docstring


def docstring_warning(
    docstring: Docstring,
    offset: int,
    message: str,
    log_level: LogLevel = LogLevel.warning,
) -> None:
    """Log a warning when parsing a docstring.

    This function logs a warning message by prefixing it with the filepath and line number.

    Parameters:
        docstring: The docstring object.
        offset: The offset in the docstring lines.
        message: The message to log.

    Returns:
        A function used to log parsing warnings if `name` was passed, else none.
    """

    def warn(docstring: Docstring, offset: int, message: str, log_level: LogLevel = LogLevel.warning) -> None:
        try:
            prefix = docstring.parent.relative_filepath  # ty:ignore[unresolved-attribute]
        except (AttributeError, ValueError):
            prefix = "<module>"
        except BuiltinModuleError:
            prefix = f"<module: {docstring.parent.module.name}>"  # ty:ignore[unresolved-attribute]
        log = getattr(logger, log_level.value)
        log(f"{prefix}:{(docstring.lineno or 0) + offset}: {message}")

    warn(docstring, offset, message, log_level)


@lru_cache(maxsize=512)
def _compile_docstring_annotation(annotation: str) -> Expression | None:
    try:
        return cast("Expression", compile(annotation, mode="eval", filename="", flags=PyCF_ONLY_AST, optimize=2))
    except SyntaxError:
        return None


def parse_docstring_annotation(
    annotation: str,
    docstring: Docstring,
    log_level: LogLevel = LogLevel.error,
) -> str | Expr:
    """Parse a string into a true name or expression that can be resolved later.

    Parameters:
        annotation: The annotation to parse.
        docstring: The docstring in which the annotation appears.
            The docstring's parent is accessed to bind a resolver to the resulting name/expression.
        log_level: Log level to use to log a message.

    Returns:
        The string unchanged, or a new name or expression.
    """
    with suppress(AttributeError):  # Docstring has no parent that can be used to resolve names.
        if (code := _compile_docstring_annotation(annotation)) and code.body:
            name_or_expr = safe_get_annotation(
                code.body,
                parent=docstring.parent,  # ty:ignore[invalid-argument-type]
                log_level=log_level,
            )
            return name_or_expr or annotation
    return annotation
