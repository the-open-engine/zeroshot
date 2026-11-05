from enum import Enum, auto


class MethodType(Enum):
    """A class to hold different method types"""

    INSTANCE_METHOD = auto()
    CLASS_METHOD = auto()
    STATIC_METHOD = auto()
