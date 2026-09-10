# Targets

A target owns source preparation and the processes that execute graph nodes; it also stores durable
run state and resolves connections. The graph protocol stays the same when the target changes.

## Local controller

Omit `--target` to work in the current Git worktree. Zeroshot starts a detached controller and keeps
the run ledger under the local state directory, where later CLI invocations can list, watch, or stop
the run.

Local execution is the only mode that mutates the caller's existing worktree; the installed Codex or
Claude harness runs as the current user.

## Direct target

A direct target exposes Zeroshot's HTTP and OECP contracts without application-level authentication.
The released container includes Zeroshot plus pinned Codex and Claude harnesses.

```console
docker run --rm --detach --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest

zeroshot target add local-target \
  --url http://127.0.0.1:8080 \
  --direct
```

!!! danger "Direct means unauthenticated"

    Keep the published port on loopback or a trusted private network. Put an authenticated reverse
    proxy with WebSocket forwarding in front of it before broader exposure.

Use `--target local-target` on run and observation commands to select it. Each named run selects
its repository and branch from the invoking worktree's attached upstream. Without an upstream, the
worktree must have exactly one GitHub remote. `--repository` and `--branch` override that selection,
and `--revision` selects an exact commit instead of resolving the current remote branch tip.
Detached worktrees require explicit repository and branch values.

## Hosted target

A hosted target adds discovery and user authentication around the same native contracts:

```console
zeroshot target add production --url https://TARGET_ORIGIN
zeroshot target login production
```

The hosting service owns login, source authorization, organization-scoped connections, queue policy,
and capacity. Zeroshot's CLI and protocol do not prescribe those product policies.

Follow the [Cloud documentation](https://dev.theopenengine.com/docs) for Zeroshot Cloud. Its pages
should link to the version of these core docs that matches the deployed target, as described in
[Documentation versions](../project/versioning.md).

## Source and delivery credentials

For named-target runs, the CLI can send `GH_TOKEN` for source checkout and generated GitHub delivery.
A provider receives that value only if its runtime binding declares `GH_TOKEN`. Before submission,
the CLI reports the target, exact `owner/repository@branch#revision`, and whether the invoking
worktree is dirty. Uncommitted files remain local and are never committed or pushed automatically.
