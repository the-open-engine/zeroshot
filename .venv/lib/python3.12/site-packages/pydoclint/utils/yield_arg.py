from dataclasses import dataclass


@dataclass
class YieldArg:
    """A class to hold one yield argument in the docstring's yields section"""

    argName: str
    argType: str
    argDescr: str
