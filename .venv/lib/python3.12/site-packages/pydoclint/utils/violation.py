from __future__ import annotations

import types
from copy import deepcopy

from pydoclint.utils.edge_case_error import EdgeCaseError

VIOLATION_CODES = types.MappingProxyType(
    {
        1: 'Potential formatting errors in docstring. Error message:',
        2: 'Syntax errors; cannot parse this Python file. Error message:',
        3: (  # noqa: PAR001
            'Docstring style mismatch. (Please read more at'
            ' https://jsh9.github.io/pydoclint/style_mismatch.html ).'
        ),
        101: 'Docstring contains fewer arguments than in function signature.',
        102: 'Docstring contains more arguments than in function signature.',
        103: (  # noqa: PAR001
            'Docstring arguments are different from function arguments.'
            ' (Or could be other formatting issues: https://jsh9.github.io/pydoclint/violation_codes.html#notes-on-doc103 ).'
        ),
        104: 'Arguments are the same in the docstring and the function signature, but are in a different order.',
        105: 'Argument names match, but type hints in these args do not match:',
        106: 'The option `--arg-type-hints-in-signature` is `True` but there are no argument type hints in the signature',
        107: 'The option `--arg-type-hints-in-signature` is `True` but not all args in the signature have type hints',
        108: 'The option `--arg-type-hints-in-signature` is `False` but there are argument type hints in the signature',
        109: 'The option `--arg-type-hints-in-docstring` is `True` but there are no type hints in the docstring arg list',
        110: 'The option `--arg-type-hints-in-docstring` is `True` but not all args in the docstring arg list have type hints',
        111: 'The option `--arg-type-hints-in-docstring` is `False` but there are type hints in the docstring arg list',
        201: 'does not have a return section in docstring',
        202: 'has a return section in docstring, but there are no return statements or annotations',
        203: 'return type(s) in docstring not consistent with the return annotation.',
        301: '__init__() should not have a docstring; please combine it with the docstring of the class',
        302: 'The class docstring does not need a "Returns" section, because __init__() cannot return anything',
        303: 'The __init__() docstring does not need a "Returns" section, because it cannot return anything',
        304: 'Class docstring has an argument/parameter section; please put it in the __init__() docstring',
        305: 'Class docstring has a "Raises" section; please put it in the __init__() docstring',
        306: 'The class docstring does not need a "Yields" section, because __init__() cannot yield anything',
        307: 'The __init__() docstring does not need a "Yields" section, because __init__() cannot yield anything',
        401: '',  # Deprecated
        402: 'has "yield" statements, but the docstring does not have a "Yields" section',
        403: (  # noqa: PAR001
            'has a "Yields" section in the docstring, but there are no "yield"'
            ' statements, or the return annotation is not a Generator/Iterator/Iterable.'
            ' (Or it could be because the function lacks a return annotation.)'
        ),
        404: 'yield type(s) in docstring not consistent with the return annotation.',
        405: (  # noqa: PAR001
            'has both "return" and "yield" statements. Please use'
            ' Generator[YieldType, SendType, ReturnType] as the return type'
            ' annotation, and put your yield type in YieldType and return type'
            ' in ReturnType. More details in'
            ' https://jsh9.github.io/pydoclint/notes_generator_vs_iterator.html'
        ),
        501: 'has raise statements, but the docstring does not have a "Raises" section',
        502: 'has a "Raises" section in the docstring, but there are not "raise" statements in the body',
        503: 'exceptions in the "Raises" section in the docstring do not match those in the function body.',
        504: 'has assert statements, but the docstring does not have a "Raises" section. (Assert statements could raise "AssertError".)',
        601: 'Class docstring contains fewer class attributes than actual class attributes.',
        602: 'Class docstring contains more class attributes than in actual class attributes.',
        603: (  # noqa: PAR001
            'Class docstring attributes are different from actual class attributes.'
            ' (Or could be other formatting issues: https://jsh9.github.io/pydoclint/violation_codes.html#notes-on-doc103 ).'
        ),
        604: 'Attributes are the same in docstring and class def, but are in a different order.',
        605: 'Attribute names match, but type hints in these attributes do not match:',
    }
)


class Violation:
    """A class to hold information of a style violation"""

    def __init__(
            self,
            line: int,
            code: int,
            msgPrefix: str = '',
            msgPostfix: str = '',
    ) -> None:
        if code not in VIOLATION_CODES:
            raise EdgeCaseError('Invalid violation code')

        self.line = line
        self.code = code
        self.violationMsg = VIOLATION_CODES[code]
        self.msgPrefix = msgPrefix
        self.msgPostfix = msgPostfix
        self.msg = ''.join(
            [
                msgPrefix,
                ' ',
                self.violationMsg,
                ' ',
                msgPostfix,
            ]
        ).strip()

    @property
    def fullErrorCode(self) -> str:
        """Full error code, including the 'DOC' prefix"""
        return 'DOC' + f'{self.code}'.zfill(3)

    def __repr__(self) -> str:
        return self.__str__()

    def __str__(self) -> str:
        return f'{self.fullErrorCode}: {self.msg}'

    def _str(self, showLineNum: bool = False) -> str:
        if not showLineNum:
            return self.__str__()

        return f'{self.line}: {self.__str__()}'

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Violation):
            return False

        return (
            self.line == other.line
            and self.code == other.code
            and self.msg == other.msg
        )

    def getInfoForFlake8(self) -> tuple[int, int, str]:
        """Get the violation info for flake8"""
        colOffset: int = 0  # we don't need column offset to locate the issue
        msg = f'{self.fullErrorCode} {self.msg}'  # no colon b/c that would cause 'yesqa' issues  # noqa: LN002
        return self.line, colOffset, msg

    def appendMoreMsg(self, moreMsg: str) -> 'Violation':
        """Append more error message, and return a new Violation object"""
        new = deepcopy(self)
        new.msg = ''.join(
            [
                new.msgPrefix,
                ' ',
                new.violationMsg,
                ' ',
                moreMsg,
                ' ',
                new.msgPostfix,
            ]
        ).strip()  # noqa: PAR104
        return new
