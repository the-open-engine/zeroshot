"""Immutable results for local named-target management."""

from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True, slots=True)
class NamedTarget:
    """A local registration, without stored login data or device identity.

    Args:
        name: Exact local target name.
        origin: Target HTTP(S) origin.
        kind: Hosted authentication or unauthenticated direct access.
    """

    name: str
    origin: str
    kind: Literal["hosted", "direct"]


@dataclass(frozen=True, slots=True)
class TargetRemoveResult:
    """Receipt for removing a local target and its saved access.

    Args:
        name: Removed target name.
        removed: True after successful cleanup and registry removal.
    """

    name: str
    removed: bool
