<div align="center">

# zeroshot

**A multi-agent orchestration framework for autonomous software development.**

[zeroshot.sh](https://zeroshot.sh/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Discord](https://theopenengine.com/discord) · [Docs](https://zeroshot.sh/docs/) · [Blog](https://theopenengine.com/blog/)

[![Release](https://img.shields.io/github/v/release/the-open-engine/zeroshot?style=flat&label=release&labelColor=171411&color=171411)](https://github.com/the-open-engine/zeroshot/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/the-open-engine/zeroshot/ci.yml?branch=main&style=flat&label=build&labelColor=171411)](https://github.com/the-open-engine/zeroshot/actions/workflows/ci.yml?query=branch%3Amain)
[![License: MIT](https://img.shields.io/badge/license-MIT-171411?style=flat)](LICENSE)

</div>

Single agents have a reliability ceiling. As context grows and complexity compounds, requirements
get buried, assumptions go unchallenged, and shortcuts start to accumulate. The agent will
optimize for completion and lose track of what correctness requires.

Independent, adversarial review fixes that. A reviewer starts with fresh
context and looks for reasons the work should fail. Rejection turns those failures into concrete
repair tasks. Each revision faces renewed scrutiny, allowing the system to recover from mistakes
that would otherwise survive all the way to “done.”

zeroshot is a multi-agent framework built around this cycle. Define your graph topology, choose
your agents and models, and make independent approval a requirement for delivery. The engine
enforces the process.

## Build your own graph

An example graph: independent reviews, end-to-end testing, and CI must pass before merge.

```text
                           ┌─────────────────┐
                           │       Goal      │
                           └────────┬────────┘
                                    ▼
┌───────────────┐          ┌─────────────────┐
│     Repair    ├─────────▶┤      Worker     │
└───────┬───────┘          └────────┬────────┘
        ▲             ┌─────────────┴─────────────┐
        │             ▼                           ▼
        │  ┌─────────────────────┐     ┌─────────────────────┐
        │  │  Acceptance review  │     │     Code review     │
        │  └──────────┬──────────┘     └──────────┬──────────┘
        │             │                           │
        │             └─────────────┬─────────────┘
        ├◀─── either rejects ───────┤
        │                           │  both approve
        │                           ▼
        │                  ┌─────────────────┐
        ├◀───── fail ──────┤ End-to-end test │
        │                  └────────┬────────┘
        │                           ▼  pass
        │                        ┌─────┐     ┌─────┐        ┌───────┐
        │                        │  PR ├────▶┤  CI ├─pass──▶┤ Merge │
        │                        └─────┘     └──┬──┘        └───────┘
        └◀───────────── fail ───────────────────┘
```

Save your graph as a [reusable profile](docs/zeroshot-cli.md#zeroshot-profile-set)
and run the same process across tasks.

See more graph examples on [zeroshot.sh](https://zeroshot.sh/#one-graph).

## Features

### Bring your own graphs

One graph per process your team runs.

- **[Graphs are JSON](docs/reference/cluster/graph.md):** steps, reviewers, branches, parallel
  work, bounded loops, and steps that repeat over a list.
- **[Profiles](docs/zeroshot-cli.md#zeroshot-profile-set):** save each graph by name and pick one
  per run with `--profile`.
- **[Agent and model per node](docs/reference/runtime-plan.md#per-node-lanes):** Claude Code,
  Codex, Copilot CLI, Pi and [more](docs/project/harness-adapters.md), with models from OpenAI,
  Anthropic, OpenRouter, Bedrock, Foundry or your own endpoint (providers vary by agent).
- **[Delivery](docs/guides/forge-delivery.md):** a run can end by pushing a branch, opening a PR,
  or opening and merging one on GitHub, GitHub Enterprise or GitLab, or by leaving the change in
  your working copy. The built-in template `software-change` defaults to PR delivery.

### The engine enforces them

Agents do the work. The graph decides what happens next.

- **[Checked before it runs](docs/concepts/execution.md#the-run-boundary):** a graph where one
  node's output doesn't fit the next node's input, a loop has no limit, or a branch can never run
  is rejected before any agent starts. `--validate-only` runs just the checks.
- **[Reviewers route the run](docs/concepts/execution.md#state-reduction-chooses-the-next-step):**
  each reviewer returns a verdict from a fixed set, and the graph says where each verdict goes.
- **[Script steps](docs/reference/runtime-plan.md#script):** run a deterministic command as a
  step in local runs.
- **[Secrets stay out of the record](docs/concepts/runtimes-and-connections.md#runtime-configuration-contains-names-not-secret-values):**
  credentials are stored once as named connections and kept out of graphs, run history, logs and
  the UI.

### See inside every run

- **[`zeroshot ui`](docs/guides/observe-and-control.md#browser):** each node's input, output and
  logs in the browser, on a timeline you can replay.
- **[CLI](docs/guides/observe-and-control.md#resume-event-and-log-streams):** `status` and `list`
  return JSON, `watch` and `logs` stream events and resume after the last one you read, `attach`
  follows a running agent.
- **[SQLite](docs/concepts/execution.md#durable-run-state):** local runs are stored in SQLite
  files you can query.
- **[Resume](docs/guides/observe-and-control.md#restart-or-resume-a-failed-run):** restart a failed
  run from a checkpoint without redoing finished steps.

### Build on it

Drive zeroshot from your own code, CI or UI.

- **[Python SDK](sdks/python/README.md):** submit, watch and wait on runs from Python. Each wheel
  bundles the matching zeroshot binary.
- **[JSON-RPC API](https://zeroshot.sh/docs/current/reference/cluster/api/):** run
  `zeroshot target serve` and connect over WebSocket to submit runs, watch their status and which
  nodes are running, and stream logs. An OpenRPC spec describes the methods.
- **[API keys](docs/zeroshot-cli.md#zeroshot-delegation):** expiring, revocable keys for CI on
  hosted targets like zeroshot Platform.
- **[Agent adapters](docs/project/harness-adapters.md):** add support for another coding agent
  through a documented contract.

## Get started

```bash
npm install -g @the-open-engine-company/zeroshot
```

This installs skills for Claude Code, Codex, Copilot CLI and Pi. Open your agent in a repo and ask:

```text
design a zeroshot graph for this repo
```

The skill reads your repo and builds the graph with you: agents, models, review gates, and
delivery. Then plan a task:

```text
plan this with zeroshot: let users export their data as CSV
```

Run the plan:

```text
zeroshot it
```

Run locally with your agent's existing login and subscription. Watch with `zeroshot ui`.
See [installation options](docs/getting-started/install.md) to skip skill installation.

### Edit a graph directly

Export a template with Git delivery disabled:

```bash
zeroshot template show software-change --delivery none > graph.json
```

Edit the graph, choose a runtime, and run it. The [review loop guide](docs/guides/review-loop.md)
has complete graph, input, and runtime files; the [CLI quick start](docs/getting-started/first-run.md)
shows validation and execution.

## Hosting

Run locally, [self-host with Docker](docker/zeroshot-target/README.md), or share graphs and run
on [zeroshot Platform](https://app.zeroshot.sh/docs) using your own model API keys.

## What users say

> I used Zeroshot for my last two contracts: a production line automation and control
> system, and a multi-host sync and admin system for Webflow and Guesty. Very happy with it!
>
> — Mark Dingwall

> you guys are solving this in a way that is more satisfying than the google ADK (to me) and it’s
> fuckin google.
>
> — Fav

## Contributing

[Development guide](CONTRIBUTING.md) · [UI development](ui/README.md) ·
[Publishing](PUBLISHING.md) · [Security](SECURITY.md) ·
[Issues](https://github.com/the-open-engine/zeroshot/issues) · [Discord](https://theopenengine.com/discord)

MIT. See [LICENSE](LICENSE).
