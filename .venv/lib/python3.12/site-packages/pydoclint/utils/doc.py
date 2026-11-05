from __future__ import annotations

import pprint
from typing import Any

from docstring_parser.common import (
    Docstring,
    DocstringReturns,
    DocstringYields,
)
from docstring_parser.google import GoogleParser
from docstring_parser.numpydoc import NumpydocParser
from docstring_parser.rest import parse as parseSphinx

from pydoclint.utils.arg import ArgList
from pydoclint.utils.edge_case_error import EdgeCaseError
from pydoclint.utils.return_arg import ReturnArg
from pydoclint.utils.yield_arg import YieldArg


class Doc:
    """A class to hold docstring and to provide info on the parsed docstring"""

    def __init__(self, docstring: str, style: str = 'numpy') -> None:
        self.docstring = docstring
        self.style = style

        parser: NumpydocParser | GoogleParser
        if style == 'numpy':
            parser = NumpydocParser()
            self.parsed = parser.parse(docstring)
        elif style == 'google':
            parser = GoogleParser()
            self.parsed = parser.parse(docstring)
        elif style == 'sphinx':
            self.parsed = parseSphinx(docstring)
        else:
            self._raiseException()

        self.docstringSize = self.parsed.size

    def __repr__(self) -> str:
        return pprint.pformat(self.__dict__, indent=2)

    @property
    def isShortDocstring(self) -> bool:  # type:ignore[return]
        """Is the docstring a short one (containing only a summary)"""
        if self.style in {'google', 'numpy', 'sphinx'}:
            # API documentation:
            # https://rr-.github.io/docstring_parser/docstring_parser.Docstring.html
            return (
                (
                    bool(self.parsed.short_description)
                    or bool(self.parsed.long_description)
                )
                and len(self.parsed.attrs) == 0
                and len(self.parsed.params) == 0
                and len(self.parsed.raises) == 0
                and self.parsed.returns is None
                and len(self.parsed.many_returns) == 0
                and self.parsed.yields is None
                and len(self.parsed.many_yields) == 0
                and len(self.parsed.examples) == 0
                and self.parsed.deprecation is None
            )

        self._raiseException()  # noqa: R503

    @property
    def argList(self) -> ArgList:  # type:ignore[return]
        """The argument info in the docstring, presented as an ArgList"""
        if self.style in {'google', 'numpy', 'sphinx'}:
            return ArgList.fromDocstringParam(self.parsed.params)

        self._raiseException()  # noqa: R503

    @property
    def attrList(self) -> ArgList:  # type:ignore[return]
        """The attributes info in the docstring, presented as an ArgList"""
        if self.style in {'google', 'numpy', 'sphinx'}:
            return ArgList.fromDocstringAttr(self.parsed.attrs)

        self._raiseException()  # noqa: R503

    @property
    def hasReturnsSection(self) -> bool:  # type:ignore[return]
        """Whether the docstring has a 'Returns' section"""
        if self.style in {'google', 'numpy', 'sphinx'}:
            retSection: DocstringReturns | None = self.parsed.returns
            return retSection is not None and not retSection.is_generator

        self._raiseException()  # noqa: R503

    @property
    def hasYieldsSection(self) -> bool:  # type:ignore[return]
        """Whether the docstring has a 'Yields' section"""
        if self.style in {'google', 'numpy', 'sphinx'}:
            yieldSection: DocstringYields = self.parsed.yields
            return yieldSection is not None

        self._raiseException()  # noqa: R503

    @property
    def hasRaisesSection(self) -> bool:  # type:ignore[return]
        """Whether the docstring has a 'Raises' section"""
        if self.style in {'google', 'numpy', 'sphinx'}:
            return len(self.parsed.raises) > 0

        self._raiseException()  # noqa: R503

    @property
    def returnSection(self) -> list[ReturnArg]:
        """Get the return section of the docstring"""
        if isinstance(self.parsed, Docstring):  # Google, numpy, Sphinx styles
            returnSection: list[DocstringReturns] = self.parsed.many_returns
            result: list[ReturnArg] = []
            for element in returnSection:
                result.append(
                    ReturnArg(
                        argName=self._str(element.return_name),
                        argType=self._str(element.type_name),
                        argDescr=self._str(element.description),
                    )
                )

            return result

        return []

    @property
    def yieldSection(self) -> list[YieldArg]:
        """Get the yield section of the docstring"""
        if isinstance(self.parsed, Docstring):  # Google, numpy, Sphinx styles
            yieldSection: list[DocstringYields] = self.parsed.many_yields
            result: list[YieldArg] = []
            for element in yieldSection:
                result.append(
                    YieldArg(
                        argName=self._str(element.yield_name),
                        argType=self._str(element.type_name),
                        argDescr=self._str(element.description),
                    )
                )

            return result

        return []

    def _raiseException(self) -> None:
        msg = f'Unknown style "{self.style}"; please contact the authors'
        raise EdgeCaseError(msg)

    @classmethod
    def _str(cls, something: Any) -> str:
        return '' if something is None else str(something)
