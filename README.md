<div align="center">

# zeroshot

**Write your QA process down as a graph. zeroshot makes every agent run follow it.**

[zeroshot.sh](https://zeroshot.sh/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Discord](https://theopenengine.com/discord) · [Docs](https://zeroshot.sh/docs/) · [Blog](https://theopenengine.com/blog/)

[![Release](https://img.shields.io/github/v/release/the-open-engine/zeroshot?style=flat&label=release&labelColor=171411&color=171411)](https://github.com/the-open-engine/zeroshot/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/the-open-engine/zeroshot/ci.yml?branch=main&style=flat&label=build&labelColor=171411)](https://github.com/the-open-engine/zeroshot/actions/workflows/ci.yml?query=branch%3Amain)
[![License: MIT](https://img.shields.io/badge/license-MIT-171411?style=flat)](LICENSE)

&nbsp;

<picture><source media="(prefers-color-scheme: dark)" srcset="docs/brand/social/starred-by-dark.png"><img alt="Starred by engineers at 300+ companies, including Google, Microsoft, Amazon, NVIDIA, Meta, GitHub and Cloudflare" src="docs/brand/social/starred-by-light.png" width="435"></picture>

</div>

Single-agent sessions degrade. Context gets buried under thousands of tokens, and the model optimizes
for "done" over "correct". In [our study](https://theopenengine.com/blog/review-loop/), all 100
sessions told to check their own work stopped after one pass.

zeroshot fixes this with independent agents that check each other's work, following a graph you
define for your repo. Reviewers start in a fresh session, can run a different model, and can't lie
about code they didn't write. Fail the check? The reason goes back to the writer, and the fix goes
through every review again until it actually works.

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

The graph is built around your repo and your QA requirements, not a fixed pipeline.

See the difference on zeroshot.sh: [a flight game](https://zeroshot.sh/#flight-demo) and
[a landing page](https://zeroshot.sh/#design-reviewer), each built by an agent alone and again through
a zeroshot graph.

## Get started

Install it once:

```bash
npm install -g @the-open-engine-company/zeroshot
```

This also installs the zeroshot skills for Claude Code, Codex, Copilot CLI and Pi. Open one of them
in your repo and ask:

```text
Use the zeroshot-graph-design skill to turn how we check changes
in this repo into a zeroshot graph.
```

It reads your build, test and CI setup, asks how you want changes delivered, and shows you the graph
and every agent's instructions before saving anything. Then it asks which models to use and saves the
graph, as your default if you want.

Then plan the work before you hand it over:

```text
plan this with zeroshot: let users export their data as CSV
```

Your agent reads the code, asks only what changes the outcome, and writes a plan where every
acceptance criterion comes with a check: a test, a command or an end-to-end scenario. You decide what
done means. Then:

```text
zeroshot it
```

The worker builds from the plan and the reviewers judge the result against those criteria. When the
run finishes, your agent tells you what got rejected and what got fixed. Watch it live with
`zeroshot ui`.

The graph also decides where finished work goes: kept local, pushed as a branch, opened as a PR,
or taken through your CI and review comments to a merge.

Runs happen on your machine with the login your agent already uses, subscriptions included. The CLI
is free and MIT licensed.

## Run it for the whole team

Ask your agent to save the graph to [zeroshot Platform](https://app.zeroshot.sh/docs) instead, and
everyone's tasks go through the same process. Platform runs work in parallel, off your laptops, on
your own model API keys.

To keep execution on your own infrastructure, run the Docker target:

```bash
docker run --detach --restart unless-stopped --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest

zeroshot target add local --url http://127.0.0.1:8080 --direct
```

## What users say

> I used Zeroshot for my last two contracts: a production line automation and control
> system, and a multi-host sync and admin system for Webflow and Guesty. Very happy with it!
>
> — Mark Dingwall

> you guys are solving this in a way that is more satisfying than the google ADK (to me) and it’s
> fuckin google.
>
> — Fav

Talk to the founders and power users in [Discord](https://theopenengine.com/discord).

## Docs

- [Build a review loop](docs/guides/review-loop.md): a complete graph with two reviewers and a repair loop
- [Graph contract](docs/reference/cluster/graph.md)
- [Delivery to GitHub and GitLab](docs/guides/forge-delivery.md): `software-change` defaults to PR delivery.
  Runs can also stay local, push a branch or merge.
- [Auto-research](docs/concepts/execution.md#built-in-templates-are-ordinary-graphs) and
  [Active Memory](docs/guides/active-memory.md)
- [CLI reference](docs/zeroshot-cli.md) and [first run from the CLI](docs/getting-started/first-run.md)
- [Target image guide](docker/zeroshot-target/README.md), [Python SDK](sdks/python/README.md),
  [Cluster API](https://zeroshot.sh/docs/current/reference/cluster/api/)

[Opcore](https://github.com/the-open-engine/opcore) gives coding agents feedback on each edit while
they write code.

## Community

Ask questions or join as a contributor in [Discord](https://theopenengine.com/discord). Report bugs
in [GitHub Issues](https://github.com/the-open-engine/zeroshot/issues).

## Development

```bash
npm ci
npm run check
cargo test --workspace # Unix; Windows: powershell -NoProfile -File scripts/test-windows.ps1
```

See [UI development](ui/README.md), [CONTRIBUTING.md](CONTRIBUTING.md),
[PUBLISHING.md](PUBLISHING.md) and [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
