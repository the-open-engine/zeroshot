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

# This module contains utilities for extracting information from parameter nodes.

from __future__ import annotations

import ast

from griffe._internal.enumerations import ParameterKind

ParametersType = list[tuple[str, ast.AST | None, ParameterKind, str | ast.AST | None]]
"""Type alias for the list of parameters of a function."""


def get_parameters(node: ast.arguments) -> ParametersType:
    parameters: ParametersType = []

    # Python stores positional-only and positional-or-keyword arguments in separate lists,
    # but stores their defaults together. The defaults are right-aligned with the combined
    # positional parameters, so `default_offset` is the index of the first parameter with
    # a default value.
    positional_count = len(node.posonlyargs) + len(node.args)
    default_offset = positional_count - len(node.defaults)

    # Keep one index across both positional argument lists to preserve that alignment.
    index = 0

    for arg in node.posonlyargs:
        default = node.defaults[index - default_offset] if index >= default_offset else None
        parameters.append((arg.arg, arg.annotation, ParameterKind.positional_only, default))
        index += 1

    for arg in node.args:
        default = node.defaults[index - default_offset] if index >= default_offset else None
        parameters.append((arg.arg, arg.annotation, ParameterKind.positional_or_keyword, default))
        index += 1

    # Variadic positional parameters have no AST default. Use an empty tuple as their
    # implicit default so they are not treated as required parameters later on.
    if node.vararg:
        parameters.append(
            (
                node.vararg.arg,
                node.vararg.annotation,
                ParameterKind.var_positional,
                "()",
            ),
        )

    # Unlike positional defaults, keyword-only defaults are stored one-to-one with their
    # arguments. A `None` entry means that the keyword-only parameter is required.
    for kwarg, kwarg_default in zip(node.kwonlyargs, node.kw_defaults, strict=False):
        parameters.append(
            (kwarg.arg, kwarg.annotation, ParameterKind.keyword_only, kwarg_default),
        )

    # Likewise, use an empty mapping as the implicit default for variadic keyword arguments.
    if node.kwarg:
        parameters.append(
            (
                node.kwarg.arg,
                node.kwarg.annotation,
                ParameterKind.var_keyword,
                "{}",
            ),
        )

    return parameters
