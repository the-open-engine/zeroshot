from dataclasses import dataclass


@dataclass
class ReturnArg:
    """A class to hold one return argument in the docstring's return section"""

    argName: str
    argType: str
    argDescr: str
