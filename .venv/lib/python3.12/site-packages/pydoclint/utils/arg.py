from __future__ import annotations

import ast
from typing import Any

from docstring_parser.common import DocstringAttr, DocstringParam

from pydoclint.utils.edge_case_error import EdgeCaseError
from pydoclint.utils.generic import (
    specialEqual,
    stripCommentsFromTypeHints,
    stripQuotes,
)
from pydoclint.utils.unparser_custom import unparseName


class Arg:
    """
    A class to hold function input/return arguments.

    This class also defines some essential behaviors of an argument, such
    as comparison, equality, hashing, etc.
    """

    def __init__(self, name: str, typeHint: str) -> None:
        if len(name) == 0:
            raise ValueError('`name` cannot be an empty string')

        self.name: str = self._removeEscapeChar(name)
        self.typeHint: str = typeHint

    def __repr__(self) -> str:
        return self.__str__()

    def __str__(self) -> str:
        return f'{self.name}: {self.typeHint}'

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Arg):
            return False

        argNamesEqual: bool = self.name == other.name
        typeHintsEqual: bool = self._typeHintsEq(self.typeHint, other.typeHint)
        return argNamesEqual and typeHintsEqual

    def __lt__(self, other: 'Arg') -> bool:
        if not isinstance(other, Arg):
            raise TypeError('Cannot compare; `other` is not of "Arg" type')

        if self.name < other.name:
            return True

        if self.name > other.name:
            return False

        return self.typeHint < other.typeHint

    def __le__(self, other: 'Arg') -> bool:
        return self < other or self == other

    def __hash__(self) -> int:
        return hash((self.name, stripQuotes(self.typeHint)))

    def nameEquals(self, other: 'Arg') -> bool:
        """More lenient equality: only compare names"""
        return self.name == other.name

    def hasTypeHint(self) -> bool:
        """Check whether this arg has type hint"""
        return self.typeHint != ''

    def isStarArg(self) -> bool:
        """Check whether this arg is a star arg (such as *args, **kwargs)"""
        return self.name.startswith('*')

    def notStarArg(self) -> bool:
        """Check whether this arg is not a star arg (*args, **kwargs)"""
        return not self.isStarArg()

    @classmethod
    def fromDocstringParam(cls, param: DocstringParam) -> 'Arg':
        """Construct an Arg object from a DocstringParam object"""
        return Arg(name=param.arg_name, typeHint=cls._str(param.type_name))

    @classmethod
    def fromDocstringAttr(cls, attr: DocstringAttr) -> 'Arg':
        """Construct an Arg object from a DocstringAttr object"""
        return Arg(name=attr.arg_name, typeHint=cls._str(attr.type_name))

    @classmethod
    def fromAstArg(cls, astArg: ast.arg) -> 'Arg':
        """Construct an Arg object from a Python AST argument object"""
        anno: ast.expr | None = astArg.annotation
        typeHint: str | None = '' if anno is None else unparseName(anno)
        assert typeHint is not None  # to help mypy better understand type
        return Arg(name=astArg.arg, typeHint=typeHint)

    @classmethod
    def fromAstArgWithMapping(
            cls,
            astArg: ast.arg,
            argToDefaultMapping: dict[ast.arg, ast.expr],
    ) -> 'Arg':
        """Construct an Arg object from AST argument with its default value"""
        anno: ast.expr | None = astArg.annotation
        typeHint: str | None = '' if anno is None else unparseName(anno)
        assert typeHint is not None  # to help mypy better understand type

        if astArg in argToDefaultMapping:
            # This means there IS a default value, even if it's None
            defaultValue = argToDefaultMapping[astArg]
            return Arg(
                name=astArg.arg,
                typeHint=f'{typeHint}, default={unparseName(defaultValue)}',
            )

        # This means there is no default value, not even a "None"
        return Arg(name=astArg.arg, typeHint=typeHint)

    @classmethod
    def fromArgWithMapping(
            cls,
            arg: 'Arg',
            argToDefaultMapping: dict[str, ast.expr],
    ) -> 'Arg':
        """Construct an Arg object from another Arg with its default value"""
        if arg.name in argToDefaultMapping:
            # This means there IS a default value, even if it's None
            defaultValue = argToDefaultMapping[arg.name]
            return Arg(
                name=arg.name,
                typeHint=f'{arg.typeHint}, default={unparseName(defaultValue)}',
            )

        # This means there is no default value, not even a "None"
        return arg

    @classmethod
    def fromAstAnnAssign(cls, astAnnAssign: ast.AnnAssign) -> 'Arg':
        """Construct an Arg object from a Python ast.AnnAssign object"""
        unparsedArgName = unparseName(astAnnAssign.target)
        unparsedTypeHint = unparseName(astAnnAssign.annotation)

        # These assertions are to help mypy better interpret types
        assert unparsedArgName is not None
        assert unparsedTypeHint is not None

        return Arg(name=unparsedArgName, typeHint=unparsedTypeHint)

    @classmethod
    def _str(cls, typeName: str | None) -> str:
        return '' if typeName is None else typeName

    @classmethod
    def _typeHintsEq(cls, hint1: str, hint2: str) -> bool:
        # We parse and then unparse so that cases like this can be
        # treated as equal:
        #
        # >>> Literal['abc', 'def', 'ghi']
        #
        # >>> Literal[
        # >>>     "abc",
        # >>>     "def",
        # >>>     "ghi",
        # >>> ]
        try:
            hint1_: str = unparseName(ast.parse(stripQuotes(hint1)))  # type:ignore[arg-type,assignment]  # noqa: LN002
        except SyntaxError:
            hint1_ = hint1

        try:
            hint2_: str = unparseName(ast.parse(stripQuotes(hint2)))  # type:ignore[arg-type,assignment]  # noqa: LN002
        except SyntaxError:
            hint2_ = hint2

        return specialEqual(hint1_, hint2_)

    @classmethod
    def _removeEscapeChar(cls, string: str) -> str:
        # We need to remove `\` from the arg names for proper comparison.
        # This is because it is often necessary to add `\` in docstrings in
        # order for Sphinx to correctly render them.
        # For example:
        #    arg1\_\_
        #    \\**kwargs
        return string.replace('\\', '')


class ArgList:
    """
    A class to hold a list of `Arg` objects.

    This class also defines some behaviors of an argument list, such as
    equality, length calculation, etc.
    """

    def __init__(self, infoList: list[Arg]) -> None:
        if not all(isinstance(_, Arg) for _ in infoList):
            raise TypeError('All elements of `infoList` must be Arg.')

        self.infoList = infoList
        self.lookup = {_.name: _.typeHint for _ in infoList}

    def __repr__(self) -> str:
        return self.__str__()

    def __str__(self) -> str:
        return '[' + ', '.join(str(_) for _ in self.infoList) + ']'

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ArgList):
            return False

        return self.infoList == other.infoList

    def __len__(self) -> int:
        return len(self.infoList)

    @property
    def isEmpty(self) -> bool:
        """Whether the arg list is empty"""
        return self.length == 0

    @property
    def nonEmpty(self) -> bool:
        """Whether the arg list is non-empty"""
        return not self.isEmpty

    @property
    def length(self) -> int:
        """Calculate the length of the list"""
        return len(self.infoList)

    @classmethod
    def fromDocstringParam(cls, params: list[DocstringParam]) -> 'ArgList':
        """Construct an ArgList from a list of DocstringParam objects"""
        infoList = [
            Arg.fromDocstringParam(_)
            for _ in params
            if _.args[0] != 'attribute'  # we only need 'param' not 'attribute'
        ]
        return ArgList(infoList=infoList)

    @classmethod
    def fromDocstringAttr(
            cls,
            params: list[DocstringAttr],
    ) -> 'ArgList':
        """Construct an ArgList from a list of DocstringAttr objects"""
        infoList = [
            Arg.fromDocstringAttr(_)
            for _ in params
            # we only need 'attribute' not 'param':
            if _.args[0] in {'attribute', 'attr'}
        ]
        return ArgList(infoList=infoList)

    @classmethod
    def fromAstAssign(cls, astAssign: ast.Assign) -> 'ArgList':
        """Construct an ArgList from variable declaration/assignment"""
        infoList: list[Arg] = []
        for i, target in enumerate(astAssign.targets):
            if isinstance(target, ast.Tuple):  # such as `a, b = c, d = 1, 2`
                for j, item in enumerate(target.elts):
                    cls._unparseTargetAndAppendToInfoList(
                        target=item,
                        infoList=infoList,
                        lineNum=astAssign.lineno,
                        endLineNum=astAssign.end_lineno,
                        i=i,
                        j=j,
                    )
            else:  # a single element
                cls._unparseTargetAndAppendToInfoList(
                    target=target,
                    infoList=infoList,
                    lineNum=astAssign.lineno,
                    endLineNum=astAssign.end_lineno,
                    i=i,
                    j=None,
                )

        return ArgList(infoList=infoList)

    @classmethod
    def _unparseTargetAndAppendToInfoList(
            cls,
            *,
            target: Any,
            infoList: list[Arg],
            lineNum: int | None,
            endLineNum: int | None,
            i: int,
            j: int | None = None,
    ) -> None:
        try:  # we may not know all potential cases, so we use try/catch
            unparsedTarget: str | None = unparseName(target)
            assert unparsedTarget is not None  # to help mypy understand type
            infoList.append(Arg(name=unparsedTarget, typeHint=''))
        except Exception as ex:
            lineRange: str = (
                f'in Line {lineNum}'
                if lineNum == endLineNum
                else f'in Lines {lineNum}-{endLineNum}'
            )
            msg1: str = f'Edge case encountered {lineRange}.'
            msg2: str = (
                f' astAssign.targets[{i}] is of type {type(target)}.'
                if j is None
                else f' astAssign.targets[{i}].elts[{j}] is of type {type(target)}.'  # noqa: LN001
            )
            msg: str = msg1 + msg2
            raise EdgeCaseError(msg) from ex

    def contains(self, arg: Arg) -> bool:
        """Whether a given `Arg` object exists in the list"""
        return arg.name in self.lookup

    def get(self, argName: str) -> Arg:
        """Retrieve an element from the list using `argName` as identifier"""
        if argName not in self.lookup:
            raise KeyError(f'argName "{argName}" not in this object')

        return Arg(name=argName, typeHint=self.lookup[argName])

    def equals(
            self,
            other: 'ArgList',
            checkTypeHint: bool = True,
            orderMatters: bool = True,
    ) -> bool:
        """
        Check whether this object is equal to `other`.

        Parameters
        ----------
        other : ArgList
            The other object
        checkTypeHint : bool
            If True, the two objects are only considered equal when type hints
            are also equal.
        orderMatters : bool
            If True, the two objects are only considered equal when the order
            of the arguments are identical.

        Returns
        -------
        bool
            Whether the two objects are equal.
        """
        if not isinstance(other, ArgList):
            return False

        if self.length != other.length:
            return False

        verdict: bool

        if checkTypeHint:
            if orderMatters:  # most strict case
                verdict = self == other
            else:
                verdict = set(self.infoList) == set(other.infoList)
        else:
            self_names = [_.name for _ in self.infoList]
            other_names = [_.name for _ in other.infoList]
            if orderMatters:
                verdict = self_names == other_names
            else:
                verdict = set(self_names) == set(other_names)

        return verdict  # noqa: R504

    def findArgsWithDifferentTypeHints(self, other: 'ArgList') -> list[Arg]:
        """Find args with unmatched type hints."""
        if not self.equals(other, checkTypeHint=False, orderMatters=False):
            msg = 'These 2 arg lists do not have the same arg names'
            raise EdgeCaseError(msg)

        result: list[Arg] = []
        for selfArg in self.infoList:
            selfArgTypeHint: str = selfArg.typeHint
            otherArgTypeHint: str = other.lookup[selfArg.name]

            # Here we use unparseName(ast.parse(...)) in order to get rid of
            # the comments at the end of type hints,
            # such as: `int, default=42  # noqa: E501`
            if not specialEqual(
                stripCommentsFromTypeHints(selfArgTypeHint),
                stripCommentsFromTypeHints(otherArgTypeHint),
            ):
                result.append(selfArg)

        return result

    def subtract(
            self,
            other: 'ArgList',
            checkTypeHint: bool = True,
    ) -> set[Arg]:
        """Find the args that are in this object but not in `other`."""
        if checkTypeHint:
            return set(self.infoList) - set(other.infoList)

        argNamesSelf = {_.name for _ in self.infoList}
        argNamesOther = {_.name for _ in other.infoList}
        diffArgName = argNamesSelf - argNamesOther
        return {Arg(name=_, typeHint=self.lookup[_]) for _ in diffArgName}

    def noTypeHints(self) -> bool:
        """Check whether none of the args have type hints"""
        return not self.hasTypeHintInAnyArg()

    def hasTypeHintInAnyArg(self) -> bool:
        """Check whether any arg has a type hint"""
        return any(_.hasTypeHint() for _ in self.infoList)

    def hasTypeHintInAllArgs(self) -> bool:
        """
        Check whether all args have a type hint.

        Star arguments (such as `*args` or `**kwargs`) are excluded because
        they don't need to have type hints.
        """
        return all(_.hasTypeHint() for _ in self.infoList if _.notStarArg())
