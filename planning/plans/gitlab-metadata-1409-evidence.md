# GitLab metadata regression validation

Historical verification on 2026-11-09 exercised draft-state normalization for merge-request
metadata and generated repair commit messages. It also checked exact source provenance,
delivery receipts and retry identity across execution and review paths.

The recorded scope included native checks and live GitLab interaction. This summary does not
publish account, project, connection or host-specific acceptance records.

The [source adapter guide](../../docs/project/source-code-adapters.md) owns the supported GitLab
configuration and behavior. Retain public regression coverage beside the native adapter.
