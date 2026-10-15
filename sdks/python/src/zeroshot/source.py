"""Immutable credential-free source provenance models."""

from dataclasses import dataclass


@dataclass(frozen=True, slots=True, kw_only=True)
class SourceForgeIdentity:
    """Repository identity owned by one source-code adapter.

    Args:
        provider: Opaque registered source provider identifier.
        repository: Opaque repository identity interpreted by that provider.
    """

    provider: str
    repository: str


@dataclass(frozen=True, slots=True, kw_only=True)
class ResolvedGitRemote:
    """Credential-free Git transport metadata.

    Args:
        url: Authored HTTPS or SSH remote locator, with its original path and username.
        forge: Optional canonical identity for PR and merge operations.
        connection: Optional named connection authorized only for Git source transport.
    """

    url: str
    forge: SourceForgeIdentity | None = None
    connection: str | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class ResolvedSource:
    """Immutable source snapshot selected for a named run.

    Args:
        repository: Native repository identity.
        branch: Attached source branch.
        revision: Exact forty- or sixty-four-character Git object ID.
        remote: Generic transport metadata; None preserves a legacy GitHub source.
    """

    repository: str
    branch: str
    revision: str
    remote: ResolvedGitRemote | None = None
