---
description: 'Contribute a harness through the same registry and feature contract used by maintained adapters.'
---

# Harness adapters

The native engine uses one harness registry. Core adapters live in
`zeroshot/src/harnesses/adapters/core/`; contributed adapters live in
`zeroshot/src/harnesses/adapters/contributed/` and register through its `register` function.
Official local builds include contributions. Platform and the default target image compile without
the `contributed-harnesses` feature and admit only core registrations.

A registration supplies a pure descriptor, provider-access declarations, native path normalization,
and a factory. Descriptor access must not read credentials, launch processes, inspect user files,
or contact a provider. The protocol stores bounded opaque harness/provider IDs; the registry owns
compatibility checks. Models remain caller-owned strings with no engine model catalog.

Factories receive host-owned launch inputs and return the existing `NodeDriver` and `SessionFactory`
interfaces. The typed host context separates local native configuration from contained process
ownership. Shared process primitives own private execution files, bounded streaming, cancellation
and confirmed cleanup. Adapters own their native wire protocol, authentication translation,
native permission handling and session identity.

Every adapter implements the same feature contract: worker and verifier responses, authoritative
local response validation and bounded corrections, reusable sessions, live output, complete or
explicitly unknown usage, cancellation, cleanup and Active Memory. Core/contributed status changes
maintenance responsibility and distribution, never runtime behavior. Native capabilities can use
different mechanisms, but must preserve those behaviors.

Active Memory supplies a frozen, scoped callback capability. Each adapter installs its native hook,
plugin or RPC bridge and acknowledges returned context. Graph guidance, retrieval budgets,
qualifying tool evidence, auxiliary-agent suppression and after-run maintenance remain shared.
Missing acknowledgement or failed transport is unknown evidence, not successful delivery.

Add an implementation folder and its registration entry. OpenCode is the first contributed example.
Run its focused fixture tests and the shared registry/session tests. Live harness tests are opt-in
and take caller-authored runtime/model/workspace selections; credentials stay in private process
environments. Contributors maintain compatibility with native harness releases.

Targets publish enabled harness/provider pairs in the separate
`openengine.runtime-harnesses/v1` discovery extension. Client preflight and server admission enforce
the same enabled catalog. Installed executables do not enable an adapter automatically, and discovery
never probes native login or provider model availability.

OpenCode supports `openai`, `anthropic`, and `openrouter` lanes. Local runs preserve native
configuration discovery through the original HOME and XDG paths, including managed settings,
global/project resources, plugins and credentialed MCP servers. Declared credentials or endpoints
add one private plugin with no package dependencies. Its native configuration and provider-model
hooks apply the selected provider's declared authentication or endpoint after OpenCode resolves its
settings. A final request-header check refuses a later plugin that replaces the declared
authentication while preserving ordinary headers and an equivalent declared credential.
The overlay masks only the selected provider's stored login in private process state; unrelated
native authentication remains available for well-known configuration discovery. Native `auth.set`
refreshes use the original file or inline authority, so refreshing another provider preserves the
selected provider's stored login.
OpenCode retains ownership of authentication-file writes.
An explicit `OPENCODE_PURE=true` conflicts with declared authentication because it disables the
required plugin.

Local OpenCode keeps native permission policy and adds no `--auto` flag. Its native configuration
inspection can start background dependency installation even in pure mode, so the adapter treats
local inspection as unavailable and uses the shared fallback. It launches no policy probe and
maintains no separate permission parser or cache-lock state. Contained runs add `--auto` inside the
disposable capsule. Active Memory installs its own tool-result plugin alongside native plugins,
while the shared broker retains recall budgets and acknowledgement receipts.
