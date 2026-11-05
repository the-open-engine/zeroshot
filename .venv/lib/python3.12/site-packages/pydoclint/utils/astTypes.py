from __future__ import annotations

import ast
import sys
from typing import Union

# typing.Union is still needed when defining custom types in Python 3.9.
# It can be changed to `xxx | yyy` after Python 3.9 is dropped.
FuncOrAsyncFuncDef = Union[ast.AsyncFunctionDef, ast.FunctionDef]
ClassOrFunctionDef = Union[ast.ClassDef, ast.AsyncFunctionDef, ast.FunctionDef]

LegacyBlockTypes = [
    ast.If,
    ast.While,
    ast.For,
    ast.AsyncFor,
    ast.With,
    ast.AsyncWith,
    ast.Try,
    ast.ExceptHandler,
]

if sys.version_info < (3, 10):
    BlockType = tuple(LegacyBlockTypes)
elif sys.version_info < (3, 11):
    BlockType = tuple(LegacyBlockTypes + [ast.match_case])
else:
    BlockType = tuple(LegacyBlockTypes + [ast.match_case, ast.TryStar])
