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

## Docs

- [Graph contract](docs/reference/cluster/graph.md) and [execution](docs/concepts/execution.md)
- [CLI reference](docs/zeroshot-cli.md), [Python SDK](sdks/python/README.md), and [Cluster API](https://zeroshot.sh/docs/current/reference/cluster/api/)
- [Delivery](docs/guides/forge-delivery.md): `software-change` defaults to PR delivery. Runs can also stay local, push a branch, or merge.
- [Active Memory](docs/guides/active-memory.md)

## Contributing

[Development guide](CONTRIBUTING.md) · [UI development](ui/README.md) ·
[Publishing](PUBLISHING.md) · [Security](SECURITY.md) ·
[Issues](https://github.com/the-open-engine/zeroshot/issues) · [Discord](https://theopenengine.com/discord)

MIT. See [LICENSE](LICENSE).
