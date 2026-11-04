<div align="center">

# zeroshot

**An open-source engine for autonomous software production.**

[zeroshot.sh](https://zeroshot.sh/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Discord](https://theopenengine.com/discord) · [Docs](https://zeroshot.sh/docs/) · [Blog](https://theopenengine.com/blog/)

[![Release](https://img.shields.io/github/v/release/the-open-engine/zeroshot?style=flat&label=release&labelColor=171411&color=171411)](https://github.com/the-open-engine/zeroshot/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/the-open-engine/zeroshot/ci.yml?branch=main&style=flat&label=build&labelColor=171411)](https://github.com/the-open-engine/zeroshot/actions/workflows/ci.yml?query=branch%3Amain)
[![License: MIT](https://img.shields.io/badge/license-MIT-171411?style=flat)](LICENSE)

&nbsp;

<picture><source media="(prefers-color-scheme: dark)" srcset="docs/brand/social/starred-by-dark.png"><img alt="Starred by engineers at 300+ companies, including Google, Microsoft, Amazon, NVIDIA, Meta, GitHub and Cloudflare" src="docs/brand/social/starred-by-light.png" width="435"></picture>

</div>

zeroshot is a framework for building and running your own multi-agent workflows. Define the
topology of your graph and choose the agents and models. The engine enforces its execution.

Think CI starting at the issue.

## Build your own graph

This graph adds a security review when a change touches authentication, payments, or migrations:

```text
worker ◄─────────── any "no" ────┐
  ▼                              │
risk check                       │
  ├─ auth, payments, migration?  │
  │    ▼                         │
  │  security review ─────── no ─┤
  ▼    ▼                         │
acceptance + code review ─── no ─┘
  ▼ both yes
deliver
```

Reviewers start in fresh sessions and can use different models. Build your own graph or adapt a
template, then save it as a [reusable profile](docs/zeroshot-cli.md#zeroshot-profile-set).

See what changes with review: [flight game](https://zeroshot.sh/#flight-demo) ·
[landing page](https://zeroshot.sh/#design-reviewer) ·
[self-review study](https://theopenengine.com/blog/review-loop/).

## Get started

```bash
npm install -g @the-open-engine-company/zeroshot
```

This installs skills for Claude Code, Codex, Copilot CLI and Pi. Open your agent in a repo and ask:

```text
design a zeroshot graph for this repo
```

It reads the repo and helps you choose the graph, models, and delivery. Then plan a task:

```text
plan this with zeroshot: let users export their data as CSV
```

Run the plan:

```text
zeroshot it
```

Watch with `zeroshot ui`. Runs happen locally using your agent's existing login, subscriptions
included. See [installation options](docs/getting-started/install.md) to skip skill installation.

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
