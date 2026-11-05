from __future__ import annotations

import ast
from typing import Callable, Generator, Type

from pydoclint.utils import walk
from pydoclint.utils.astTypes import BlockType, FuncOrAsyncFuncDef
from pydoclint.utils.generic import stringStartsWith
from pydoclint.utils.unparser_custom import unparseName

ReturnType = Type[ast.Return]
ExprType = Type[ast.expr]
YieldAndYieldFromTypes = tuple[Type[ast.Yield], Type[ast.YieldFrom]]
FuncOrAsyncFuncTypes = tuple[Type[ast.FunctionDef], Type[ast.AsyncFunctionDef]]
FuncOrAsyncFunc = (ast.FunctionDef, ast.AsyncFunctionDef)


def hasReturnAnnotation(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the function node has a return type annotation"""
    return node.returns is not None


def isReturnAnnotationNone(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the return type annotation if `None`"""
    return _isNone(node.returns)


def _isNone(node: ast.expr | None) -> bool:
    return isinstance(node, ast.Constant) and node.value is None


def isReturnAnnotationNoReturn(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the return type annotation if `NoReturn`"""
    if node.returns is None:
        return False

    returnAnnotation: str | None = unparseName(node.returns)
    return returnAnnotation == 'NoReturn'


def hasGeneratorAsReturnAnnotation(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the function node has a 'Generator' return annotation"""
    if node.returns is None:
        return False

    returnAnno: str | None = unparseName(node.returns)
    return returnAnno in {'Generator', 'AsyncGenerator'} or stringStartsWith(
        returnAnno, ('Generator[', 'AsyncGenerator[')
    )


def hasIteratorOrIterableAsReturnAnnotation(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether `node` has a 'Iterator' or 'Iterable' return annotation"""
    if node.returns is None:
        return False

    returnAnnotation: str | None = unparseName(node.returns)
    return returnAnnotation in {
        'Iterator',
        'Iterable',
        'AsyncIterator',
        'AsyncIterable',
    } or stringStartsWith(
        returnAnnotation,
        ('Iterator', 'Iterable', 'AsyncIterator', 'AsyncIterable'),
    )


def hasYieldStatements(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the function node has any yield statements"""

    def isThisNodeAYieldStmt(node_: ast.AST) -> bool:
        return isinstance(node_, ast.Expr) and isinstance(
            node_.value, (ast.Yield, ast.YieldFrom)
        )

    return _hasExpectedStatements(node, isThisNodeAYieldStmt)


def hasReturnStatements(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the function node has any return statements"""

    def isThisNodeAReturnStmt(node_: ast.AST) -> bool:
        return isinstance(node_, ast.Return)

    return _hasExpectedStatements(node, isThisNodeAReturnStmt)


def hasBareReturnStatements(node: FuncOrAsyncFuncDef) -> bool:
    """
    Check whether the function node has bare return statements (i.e.,
    just a "return" without anything behind it)
    """

    def isThisNodeABareReturnStmt(node_: ast.AST) -> bool:
        return isinstance(node_, ast.Return) and node_.value is None

    return _hasExpectedStatements(node, isThisNodeABareReturnStmt)


def hasRaiseStatements(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the function node has any raise statements"""

    def isThisNodeARaiseStmt(node_: ast.AST) -> bool:
        return isinstance(node_, ast.Raise)

    return _hasExpectedStatements(node, isThisNodeARaiseStmt)


def hasAssertStatements(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether the function node has any assert statements"""

    def isThisNodeAnAssertStmt(node_: ast.AST) -> bool:
        return isinstance(node_, ast.Assert)

    return _hasExpectedStatements(node, isThisNodeAnAssertStmt)


def getRaisedExceptions(
        node: FuncOrAsyncFuncDef,
        shouldDeclareAssertError: bool = False,
) -> list[str]:
    """Get the raised exceptions in a function node as a sorted list"""
    return sorted(set(_getRaisedExceptions(node, shouldDeclareAssertError)))


def _getRaisedExceptions(  # noqa: C901
        node: FuncOrAsyncFuncDef,
        shouldDeclareAssertError: bool = False,
) -> Generator[str, None, None]:
    """Yield the raised exceptions or asserts in a function node"""
    childLineNum: int = -999

    # key: child lineno, value: (parent lineno, is parent a function?)
    familyTree: dict[int, tuple[int, bool]] = {}

    currentParentExceptHandler: ast.ExceptHandler | None = None

    exceptionName: str | None

    # Depth-first guarantees the last-seen exception handler
    # is a parent of child.
    for child, parent in walk.walk_dfs(node):
        childLineNum = _updateFamilyTree(child, parent, familyTree)

        if isinstance(child, ast.Assert) and shouldDeclareAssertError:
            yield 'AssertionError (implicitly from the `assert` statement)'

        if isinstance(parent, ast.ExceptHandler):
            currentParentExceptHandler = parent

        if (
            isinstance(child, ast.Raise)
            and isinstance(
                parent,
                (ast.AsyncFunctionDef, ast.FunctionDef, BlockType),
            )
            and _confirmThisStmtIsNotWithinNestedFunc(
                foundStatementTemp=True,
                familyTree=familyTree,
                lineNumOfStatement=childLineNum,
                lineNumOfThisNode=node.lineno,
            )
        ):
            for subnode, _ in walk.walk_dfs(child):
                if isinstance(subnode, ast.Name):
                    if isinstance(child.exc, ast.Attribute):
                        # case: looks like m.n.exception
                        exceptionName = unparseName(child.exc)
                        assert isinstance(exceptionName, str)  # satisfy mypy
                        yield exceptionName
                    elif isinstance(child.exc, ast.Call) and isinstance(
                        child.exc.func, ast.Attribute
                    ):
                        # case: looks like m.n.exception()
                        exceptionName = unparseName(child.exc.func)
                        assert isinstance(exceptionName, str)  # satisfy mypy
                        yield exceptionName
                    elif (
                        currentParentExceptHandler
                        and currentParentExceptHandler.name
                        and subnode.id == currentParentExceptHandler.name
                    ):
                        # case: "except <> as e; raise e" -> we must yield the
                        # stuff in <>
                        #
                        # Note:
                        # if subnode.id != currentParentExceptHandler.name,
                        # the user is raising something not bound by
                        # this exception handler (meaning we should fall
                        # through to yielding the subnode.id)
                        yield from _extractExceptionsFromExcept(
                            currentParentExceptHandler
                        )
                    else:
                        yield subnode.id

                    break
            else:
                # if "raise" statement was alone, it must be inside an "except"
                if currentParentExceptHandler:
                    yield from _extractExceptionsFromExcept(
                        currentParentExceptHandler,
                    )


def _extractExceptionsFromExcept(
        node: ast.ExceptHandler,
) -> Generator[str, None, None]:
    if isinstance(node.type, ast.Name):
        yield node.type.id

    exceptionName: str | None

    if isinstance(node.type, ast.Attribute):
        # case: looks like m.n.exception
        exceptionName = unparseName(node.type)
        assert isinstance(exceptionName, str)  # to make mypy happy
        yield exceptionName

    if isinstance(node.type, ast.Tuple):
        for elt in node.type.elts:
            if isinstance(elt, ast.Attribute):
                # case: looks like m.n.exception
                exceptionName = unparseName(elt)
                assert isinstance(exceptionName, str)  # to make mypy happy
                yield exceptionName
            elif isinstance(elt, ast.Name):
                yield elt.id


def _hasExpectedStatements(
        node: FuncOrAsyncFuncDef,
        isThisNodeAnExpectedStmt: Callable[[ast.AST], bool],
) -> bool:
    """
    Check whether the node contains an expected statement (return, yield,
    raise, or assert).
    """
    childLineNum: int = -999
    foundExpectedStmt: bool = False

    # key: child lineno, value: (parent lineno, is parent a function?)
    familyTree: dict[int, tuple[int, bool]] = {}

    for child, parent in walk.walk(node):
        childLineNum = _updateFamilyTree(child, parent, familyTree)

        if isThisNodeAnExpectedStmt(child):
            if isinstance(parent, (ast.AsyncFunctionDef, ast.FunctionDef)):
                foundExpectedStmt = True
                break

            if isinstance(parent, BlockType):
                foundExpectedStmt = True
                break

    return _confirmThisStmtIsNotWithinNestedFunc(
        foundStatementTemp=foundExpectedStmt,
        familyTree=familyTree,
        lineNumOfStatement=childLineNum,
        lineNumOfThisNode=node.lineno,
    )


def _updateFamilyTree(
        child: ast.AST,
        parent: ast.AST,
        familyTree: dict[int, tuple[int, bool]],
) -> int:
    """
    Structure of `familyTree`:
        Key: line number of child node
        Value: (line number of parent node, whether this parent is a function)
    """
    childLine = _getLineNum(child)
    parentLine = _getLineNum(parent)
    if childLine != -1 and parentLine != -1 and childLine != parentLine:
        isFunction = isinstance(parent, FuncOrAsyncFunc)
        familyTree[childLine] = (parentLine, isFunction)

    return childLine


def _getLineNum(node: ast.AST) -> int:
    lineNum: int
    try:
        if 'lineno' in node.__dict__:  # normal case
            lineNum = node.lineno  # type:ignore[attr-defined]
        elif 'pattern' in node.__dict__:  # the node is a `case ...:`
            lineNum = node.pattern.lineno  # type:ignore[attr-defined]
        else:  # fallback case, but this could still fail
            lineNum = node.lineno  # type:ignore[attr-defined]
    except AttributeError:  # if `node` doesn't have any of those attributes
        lineNum = -1

    return lineNum


def _confirmThisStmtIsNotWithinNestedFunc(
        foundStatementTemp: bool,
        familyTree: dict[int, tuple[int, bool]],
        lineNumOfStatement: int,
        lineNumOfThisNode: int,
) -> bool:
    """
    Check whether we REALLY found the expected statement (return, yield,
    raise, or assert).

    Returns True if this statement is not within a nested function of `node`.
    Returns False if otherwise.

    We do this by checking whether the line number of the parent function
    of the statement is the same as the line number of the node.
    """
    if not foundStatementTemp:
        return False

    parentFuncLineNum = _lookupParentFunc(familyTree, lineNumOfStatement)
    return parentFuncLineNum == lineNumOfThisNode


def _lookupParentFunc(
        familyLine: dict[int, tuple[int, bool]],
        lineNumOfChildNode: int,
) -> int:
    """
    Look up the parent function of the given child node.

    Recursion is used in this function, because the key-val pairs in
    `familyLine` only records immediate child-parent mapping.
    """
    if lineNumOfChildNode not in familyLine:
        return -999

    parentLineNum, isParentAFunction = familyLine[lineNumOfChildNode]

    if isParentAFunction:
        return parentLineNum

    return _lookupParentFunc(familyLine, parentLineNum)
