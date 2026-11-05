from __future__ import annotations

import ast
import re
import sys

from pydoclint.utils.edge_case_error import EdgeCaseError


def replaceTupleBracket(string: str) -> str:
    """
    Remove the comma in strings like "tuple[*Shape,]"

    We need to do this at least for Python 3.11, because when we write
    type annotations such as "tuple[*Shape]", the ast.unparse() returns
    "tuple[*Shape,]" (one more comma).
    """
    return re.sub(r'(tuple|Tuple)\[\*(.*?),\]', r'\1[*\2]', string)


if sys.version_info >= (3, 11):

    def py311unparse(astObj: ast.AST) -> str:
        """Unparse an AST tree"""
        return replaceTupleBracket(ast.unparse(astObj))

    unparse = py311unparse
elif sys.version_info >= (3, 9):
    unparse = ast.unparse
else:  # python 3.8
    raise EdgeCaseError('pydoclint does not support Python 3.8 any more')


def unparseName(
        node: ast.expr | ast.Module | None,
) -> str | None:
    """Parse type annotations from argument list or return annotation."""
    if node is None:
        return None

    return unparse(node).strip()
