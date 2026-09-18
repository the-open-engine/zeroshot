---
name: zeroshot
description: Use Zeroshot to prepare, run, observe, or troubleshoot explicit multi-agent software work locally or on Zeroshot Cloud. Apply when the user names Zeroshot or asks about its CLI; do not route ordinary local work into Zeroshot without that intent.
---

# Zeroshot

Zeroshot executes an explicit graph of software agents. Workers make changes; independent verifiers
judge the evidence, and the graph decides when to repair or deliver. A successful run means the
graph reached its declared outcome; it does not always mean a pull request was created or merged.

Use the installed CLI help as the exact contract when commands differ from this overview.

## Shape the task

Before submitting, inspect the request, repository guidance, existing issue, and enough code to know:

- the desired behavior and observable success;
- material constraints and behavior that must remain unchanged;
- whether a separate worker could implement it and a verifier could judge it.

Fill gaps from available evidence. Choose routine technical details without interrogating the user.
If an unresolved product choice would materially change the outcome, propose the narrowest supported
interpretation and ask one focused question. Do not submit while that decision remains unanswered.
Suggest separate tasks only for independently useful outcomes or real prerequisites.

Never start a Zeroshot run merely because the skill is available. The user must ask to use Zeroshot.
A worker already executing inside a Zeroshot run should perform its assigned work directly, not start
another run. A failed Cloud run does not authorize silently implementing the task locally.

## Choose the target

- Local execution omits `--target` and is the only mode that edits the invoking worktree directly.
- Zeroshot Cloud is built in. Sign in once with `zeroshot target login cloud`, then use
  `--target cloud` explicitly on Cloud commands.
- Do not add or set up the built-in Cloud target. Named targets store endpoints and login identity,
  not repository configuration.

## Reuse profiles and connections

Prefer a saved profile over inventing a graph or runtime. Inspect its input, runtime, and delivery
behavior before constructing a run:

```console
zeroshot profile list --target cloud --scope user
zeroshot profile list --target cloud --scope org
zeroshot profile show NAME --target cloud --scope org
zeroshot connection list --target cloud --scope user
zeroshot connection list --target cloud --scope org
```

Do not assume a profile creates a pull request. `--pr` requests pull-request delivery and `--ship`
requests merge delivery when materializing a compatible template; use either only when intended.

## Validate and submit

Put profile input in a JSON file matching the profile's declared schema. Use a stable, non-secret
submission key for safe retries:

```console
zeroshot run --title "TASK" --profile org:NAME --input input.json \
  --target cloud --submission-key KEY --validate-only

zeroshot run --title "TASK" --profile org:NAME --input input.json \
  --target cloud --submission-key KEY
```

`--validate-only` materializes and validates without starting a run. A named Cloud profile must
still be fetched from the target first, so this command requires Cloud access. It does not prove
connection availability.

For named targets, Zeroshot resolves source from the invoking Git worktree's attached upstream.
Without an upstream, it requires exactly one GitHub remote. Detached worktrees require explicit
repository and branch values. Override source only when needed:

```console
--target cloud --repository OWNER/REPOSITORY --branch BRANCH --revision SHA
```

Cloud receives the remote revision, not uncommitted files. Check relevant dirty or unpushed work and
never push or omit it without authorization. Confirm the exact repository, branch, revision, target,
profile, and delivery intent reported for the run.

## Observe and troubleshoot

Keep the run ID and target:

```console
zeroshot status RUN_ID --target cloud
zeroshot watch RUN_ID --target cloud
zeroshot logs RUN_ID --target cloud
```

A foreground run streams events. Ctrl-C detaches observation without stopping the run. Use
`zeroshot force-stop RUN_ID --target cloud` only when the user explicitly intends to stop it.

For command errors, check `zeroshot --version` and the relevant `--help`; do not revive removed
commands such as `target setup`. If login cannot retain credentials, fix the credential store before
requesting another device code; `zeroshot target login --help` lists the supported
`ZEROSHOT_CREDENTIAL_STORE` modes.

For `connection_unavailable`, list connections for the same target and both relevant scopes. A
static `connection set` replaces the whole named connection, so supply every required field through
repeated `--field` prompts or JSON stdin. Never put secret values in arguments, task files, runtime
configuration, issues, or logs.

After an uncertain submission response, reconcile by submission key before retrying. Do not create a
second logical run. Report submitted, running, verified, delivered, failed, and stopped states
accurately; submission alone is not completion.
