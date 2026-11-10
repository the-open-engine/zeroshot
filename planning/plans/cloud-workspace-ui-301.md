# Shared workspace UI

The same editor and run viewer support standalone use and embedding through `embed.html`.
Embedding hosts supply authentication, persistence and bounded history services through the
published workspace interfaces. Host implementation details stay outside the shared UI.

The bridge binds one same-origin parent/iframe pair to an opaque workspace identity. Document
snapshots and save acknowledgements retain their generation and request identity; navigation
preserves unsaved work. Observation follows recorded history without starting a controller.

The [UI guide](../../ui/README.md#platform-integration) owns the embedding, authoring and history
contracts. See [standalone verification](../../ui/qa/standalone-integration.md) for native UI
coverage and [workspace verification](cloud-workspace-ui-301-evidence.md) for its historical scope.
