from __future__ import annotations

import ast
import json

from pydoclint.utils.edge_case_error import EdgeCaseError
from pydoclint.utils.generic import stripQuotes
from pydoclint.utils.unparser_custom import unparseName


class ReturnAnnotation:
    """A class to hold the return annotation in a function's signature"""

    def __init__(self, annotation: str | None) -> None:
        self.annotation: str | None = stripQuotes(annotation)

    def __str__(self) -> str:
        return f'ReturnAnnotation(annotation={json.dumps(self.annotation)})'

    def __repr__(self) -> str:
        return self.__str__()

    def decompose(self) -> list[str]:
        """
        Numpy style allows decomposing the returning tuple into individual
        element.  For example, if the return annotation is `Tuple[int, bool]`,
        you can put 2 return values in the return section: int, and bool.

        This method decomposes such return annotation into individual elements.

        Returns
        -------
        list[str]
            The decomposed element

        Raises
        ------
        EdgeCaseError
            When the annotation string has strange values
        """
        if self._isTuple():  # noqa: R506
            assert self.annotation is not None  # to help mypy understand type

            if not self.annotation.endswith(']'):
                raise EdgeCaseError('Return annotation not ending with `]`')

            if len(self.annotation) < 7:
                raise EdgeCaseError(f'Impossible annotation {self.annotation}')

            if self.annotation.lower() == 'tuple[]':
                return []

            insideTuple: str = self.annotation[6:-1]
            if insideTuple.endswith('...'):  # like this: Tuple[int, ...]
                # because we don't know the tuple's length
                return [self.annotation]

            parsedBody0: ast.Expr = ast.parse(insideTuple).body[0]  # type:ignore[assignment]  # noqa: LN002
            if isinstance(
                parsedBody0.value, (ast.Attribute, ast.Name)
            ):  # such as Tuple[int]
                return [insideTuple]

            if isinstance(
                parsedBody0.value, ast.Tuple
            ):  # like Tuple[int, str]
                elts: list[ast.expr] = parsedBody0.value.elts
                return [unparseName(_) for _ in elts]  # type:ignore[misc]

            raise EdgeCaseError('decompose(): This should not have happened')
        else:
            return self.putAnnotationInList()

    def _isTuple(self) -> bool:
        try:
            assert self.annotation is not None  # to help mypy understand type
            annoHead = ast.parse(self.annotation).body[0].value.value.id  # type:ignore[attr-defined]  # noqa: LN002
            return annoHead in {'tuple', 'Tuple'}
        except Exception:
            return False

    def putAnnotationInList(self) -> list[str]:
        """Put annotation string in a list"""
        return [] if self.annotation is None else [self.annotation]
