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

Add an implementation folder and its registration entry. OpenCode and Hermes provide contributed examples.
Run its focused fixture tests and the shared registry/session tests. Live harness tests are opt-in
and take caller-authored runtime/model/workspace selections; credentials stay in private process
environments. Contributors maintain compatibility with native harness releases.
Native CI also runs the Hermes bridge's Python unit tests without installing a harness. To include
its SDK fixtures, use Hermes's Python environment and set `HERMES_SOURCE` to its source checkout:

```bash
python -m unittest discover -s zeroshot/src/harnesses/adapters/contributed/hermes/tests -p 'native_*_test.py'
```

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

Hermes supports the same `openai`, `anthropic`, and `openrouter` lanes. Install Hermes separately;
the adapter uses the interpreter selected by its canonical `hermes` executable. It uses Hermes's
native Python agent and configuration factories in an owned process, preserving profile selection,
managed settings, enabled plugins, MCP, native instructions and local permission policy. Compatibility
has been exercised against Hermes v0.22.5. Contributors maintain compatibility with newer native releases.

A private import hook retains native startup and hands off to the adapter before the CLI starts its
shared gateway. If the launcher ignores the hook, `--version --help` exits without consuming a model
prompt. Credentials cross private stdin after a versioned handshake. Declared credentials retain
authority across native model-client rebuilds, auxiliary calls and delegated agents; conflicting
provider routes fail before authentication. Native login keeps Hermes's refresh behavior.

Each execution owns one native agent runtime. Corrections reuse it, and node revisits resume a private
native session database through Hermes's compression-tip resolver. The original native profile stays
bound through execution and cleanup, preserving SOUL and skills lookup. Captured shell values stay private
until native configuration requests them. Declared connections override native dotenv values; provider
credentials are available to explicit native configuration lookups without entering the tool environment.
On cancellation the adapter interrupts native work and waits for Hermes to
clean up its detached terminal jobs, Python kernels, language servers and MCP processes before falling back to process termination.
Missing native cleanup acknowledgement makes cleanup unconfirmed. Native usage deltas remain visible;
unreported auxiliary or child work makes totals incomplete. Active Memory adds model-facing native
notices after tool-result bookkeeping, so native plugin transforms and persisted messages still work.
Remote backends retain Hermes's native process boundary. Checked native transport confirms known
kernel ownership and reports uncertainty when settlement cannot be established.
