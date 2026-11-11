<div align="center">

# zeroshot

**A multi-agent orchestration engine that keeps coding agents from grading their own work.**

[zeroshot.sh](https://zeroshot.sh/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Docs](https://zeroshot.sh/docs/) · [Research blog](https://zeroshot.sh/blog/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Discord](https://theopenengine.com/discord)

</div>

```bash
npm install -g @the-open-engine-company/zeroshot
```

Single agents have a reliability ceiling. As context grows and complexity compounds, requirements
get buried, assumptions go unchallenged, and shortcuts start to accumulate. The agent will
optimize for completion and lose track of what correctness requires.

Independent, adversarial review fixes that, because a reviewer starts with fresh
context and looks for reasons the work should fail. Rejection turns those failures into concrete
repair tasks. Each revision faces renewed scrutiny, allowing the system to recover from mistakes
that would otherwise survive all the way to “done.”

zeroshot is a multi-agent framework built around this cycle. Define your graph topology, choose
your agents and models, and make independent approval a requirement for delivery. The engine
enforces the process.

## How it works

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

The CLI installer adds skills for Claude Code, Codex, Copilot CLI and Pi.
See [installation options](docs/getting-started/install.md) to skip skill installation.

Use zeroshot in ChatGPT, Codex, or Claude with the [plugin and MCP connection](docs/guides/plugins.md);
the zeroshot CLI is optional for Platform workflows.

### Run a first task

Open your agent in a repo and ask for a change. For example:

```text
use zeroshot to let users export their data as CSV. Keep the changes local.
```

Run locally with your agent's existing login and subscription. Watch with `zeroshot ui`.
The [CLI quick start](docs/getting-started/first-run.md) walks through a run directly in the terminal.

### Design your own graph

To choose your own agents, models, review gates, and delivery, ask:

```text
design a zeroshot graph for this repo
```

The skill reads your repo and builds the graph with you. Then plan a task:

```text
plan this with zeroshot: let users export their data as CSV
```

Run the plan:

```text
zeroshot it
```

#### Edit a graph directly

Export a template with Git delivery disabled:

```bash
zeroshot template show software-change --delivery none > graph.json
```

Edit the graph, choose a runtime, and run it. The [review loop guide](docs/guides/review-loop.md)
has complete graph, input, and runtime files.

## Features

### Graphs and agents

One graph per process your team runs.

- **[Graphs are JSON](docs/reference/cluster/graph.md):** steps, reviewers, branches, parallel
  work, bounded loops and maps over lists.
- **[Profiles](docs/zeroshot-cli.md#zeroshot-profile-set):** save each graph by name and pick one
  per run with `--profile`.
- **[Agent and model per node](docs/reference/runtime-plan.md#per-node-lanes):** Claude Code,
  Codex, Copilot CLI, Pi and [more](docs/project/harness-adapters.md), through the providers each
  agent supports: OpenAI, Anthropic, OpenRouter, Bedrock, Foundry or your own endpoint.
- **[Delivery](docs/guides/forge-delivery.md):** a run can end by pushing a branch, opening a PR,
  or opening and merging one on GitHub, GitHub Enterprise or GitLab, or by leaving the change in
  your working copy. The built-in template `software-change` defaults to PR delivery.

### The engine enforces them

Agents do the work. The graph decides what happens next.

- **[Checked before it runs](docs/concepts/execution.md#the-run-boundary):** type mismatches,
  unbounded loops and unreachable branches are rejected before any agent starts.
  `--validate-only` runs just the checks.
- **[Reviewers route the run](docs/concepts/execution.md#state-reduction-chooses-the-next-step):**
  each reviewer returns an enum verdict, and the graph routes on it.
- **[Script steps](docs/reference/runtime-plan.md#script):** run a deterministic command as a
  step in local runs.
- **[Secrets stay out of the record](docs/concepts/runtimes-and-connections.md#runtime-configuration-contains-names-not-secret-values):**
  credentials are stored once as named connections and kept out of graphs, run history, logs and
  the UI.

### See inside every run

- **[`zeroshot ui`](docs/guides/observe-and-control.md#browser):** each node's input, output and
  logs in the browser, on a timeline you can replay.
- **[CLI](docs/guides/observe-and-control.md#resume-event-and-log-streams):** `status` and `list`
  return JSON, `watch` and `logs` stream resumable events, `attach` follows a running agent.
- **[SQLite](docs/concepts/execution.md#durable-run-state):** one SQLite file per local run.
- **[Resume](docs/guides/observe-and-control.md#restart-or-resume-a-failed-run):** restart a failed
  run from a checkpoint without redoing finished steps.

### Build on it

Drive zeroshot from your own code, CI or UI.

- **[Python SDK](sdks/python/README.md):** async Python client for submitting, watching and
  waiting on runs. Each wheel bundles the matching zeroshot binary.
- **[JSON-RPC API](https://zeroshot.sh/docs/current/reference/cluster/api/):** run
  `zeroshot target serve` and connect over WebSocket to submit runs, watch their status and which
  nodes are running, and stream logs. An OpenRPC spec describes the methods.
- **[API keys](docs/zeroshot-cli.md#zeroshot-delegation):** expiring, revocable CI keys for hosted
  targets.
- **[Agent adapters](docs/project/harness-adapters.md):** add support for another coding agent
  through a documented contract.

## Hosting

Run locally, [self-host with Docker](docker/zeroshot-target/README.md), or share graphs and run
on [zeroshot Platform](https://app.zeroshot.sh/docs) using your own model API keys.

## Community

[The Open Engine](https://theopenengine.com/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Research blog](https://zeroshot.sh/blog/?utm_source=github&utm_medium=readme&utm_campaign=zeroshot) · [Discord](https://theopenengine.com/discord) · [X](https://x.com/OpenEngineHQ) · [LinkedIn](https://www.linkedin.com/company/the-open-engine-company/) · [Instagram](https://www.instagram.com/theopenengine/)

## Contributing

[Development guide](CONTRIBUTING.md) · [UI development](ui/README.md) ·
[Publishing](PUBLISHING.md) · [Security](SECURITY.md) ·
[Issues](https://github.com/the-open-engine/zeroshot/issues) · [Discord](https://theopenengine.com/discord)

MIT. See [LICENSE](LICENSE).
