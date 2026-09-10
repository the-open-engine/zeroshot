"""Targets, graphs, and runtimes accepted by the Zeroshot Python SDK."""

from __future__ import annotations

from collections.abc import Mapping
from copy import deepcopy
from dataclasses import dataclass, field
from os import PathLike
from typing import TypeAlias

from .values import JsonValue


@dataclass(frozen=True, slots=True)
class LocalTarget:
    """Execute against the local Zeroshot controller.

    Args:
        workspace: Git workspace used in place by new runs. None captures the current directory
            when the client opens.
        state_dir: Optional native controller state directory. Reuse it to observe local runs from
            later client instances; None uses the native package-owned default.
    """

    workspace: str | PathLike[str] | None = None
    state_dir: str | PathLike[str] | None = field(default=None, kw_only=True)


@dataclass(frozen=True, slots=True)
class DirectTarget:
    """Connect to an unauthenticated Zeroshot target, including the Docker image.

    Args:
        origin: Target HTTP(S) origin. Native validation permits plain HTTP only on loopback.
        workspace: Git worktree used to select source and report dirty state. None captures the
            current directory when the client opens.
    """

    origin: str
    workspace: str | PathLike[str] | None = field(default=None, kw_only=True)


@dataclass(frozen=True, slots=True)
class HostedTarget:
    """Use a named hosted target already configured and logged in through the CLI.

    Args:
        name: Exact local target name from ``zeroshot target list``. The SDK reuses that
            target's stored origin and login.
    """

    name: str


Target: TypeAlias = LocalTarget | DirectTarget | HostedTarget


@dataclass(frozen=True, slots=True)
class Preset:
    """Select an executable-owned built-in graph template.

    Args:
        name: Exact name returned by Client.list_presets().
        delivery: Native delivery selector: none, pull_request, or merge.
    """

    name: str
    delivery: str = field(default="none", kw_only=True)


@dataclass(frozen=True, slots=True)
class _OpaqueDocument:
    document: Mapping[str, JsonValue]

    def __post_init__(self) -> None:
        object.__setattr__(self, "document", deepcopy(dict(self.document)))

    def to_dict(self) -> dict[str, JsonValue]:
        return deepcopy(dict(self.document))


@dataclass(frozen=True, slots=True)
class GraphSpec(_OpaqueDocument):
    """Opaque custom GraphSpec passed unchanged to Zeroshot.

    Args:
        document: JSON-compatible GraphSpec mapping. Python performs no semantic validation.
    """

    @classmethod
    def from_dict(cls, value: Mapping[str, JsonValue]) -> GraphSpec:
        """Construct an opaque GraphSpec without validating or traversing it."""
        return cls(value)

    def to_dict(self) -> dict[str, JsonValue]:
        """Return a defensive mutable copy suitable for native JSON encoding."""
        return _OpaqueDocument.to_dict(self)


@dataclass(frozen=True, slots=True, kw_only=True)
class UniformRuntime:
    """Apply one agent runtime to every executable graph node.

    Args:
        harness: Native codex or claude harness name.
        provider: Native provider name.
        model: Native model identifier.
        effort: Optional native reasoning effort.
        size: Native small, medium, or large run size.
        session_scope: execution opens a fresh session for each execution; node_instance reuses a
            live session when that graph node instance runs again.
        connections: Connection keys mapped to the exact environment field names their agent nodes
            need. None selects the executable-owned provider defaults; an empty mapping declares no
            connections. Values are read only from Client.environment.
    """

    harness: str
    provider: str
    model: str
    effort: str | None = None
    size: str = "medium"
    session_scope: str = "execution"
    connections: Mapping[str, tuple[str, ...]] | None = None

    def to_dict(self) -> dict[str, JsonValue]:
        """Encode the declarative uniform runtime consumed and validated by Zeroshot."""
        value: dict[str, JsonValue] = {
            "provider": self.provider,
            "model": self.model,
            "size": self.size,
            "sessionScope": self.session_scope,
        }
        value["harness"] = self.harness
        if self.effort is not None:
            value["effort"] = self.effort
        if self.connections is not None:
            value["connections"] = {
                key: list(environment) for key, environment in self.connections.items()
            }
        return value


@dataclass(frozen=True, slots=True)
class RuntimePlan(_OpaqueDocument):
    """Opaque native RuntimePlan passed unchanged to Zeroshot.

    Args:
        document: JSON-compatible RuntimePlan mapping. Python performs no semantic validation.
    """

    @classmethod
    def from_dict(cls, value: Mapping[str, JsonValue]) -> RuntimePlan:
        """Construct an opaque RuntimePlan without validating graph bindings."""
        return cls(value)

    def to_dict(self) -> dict[str, JsonValue]:
        """Return a defensive mutable copy suitable for native JSON encoding."""
        return _OpaqueDocument.to_dict(self)
