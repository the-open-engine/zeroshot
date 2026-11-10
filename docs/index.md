---
description: 'The engine for autonomous software production. Orchestrate coding agents through implementation, code review, testing, repair and delivery.'
---

# The engine for autonomous software production

<p class="docs-lede">
zeroshot is an open-source workflow engine for coding agents. Write your team's development and
QA process as a graph of implementation, independent review, testing, repair and delivery. Agents
do the work; the graph decides what happens next, and the engine enforces its transitions and
retry limits.
</p>

<figure class="docs-example-graph">
--8<-- "overrides/partials/example-graph-wide.svg"
--8<-- "overrides/partials/example-graph-tall.svg"
<figcaption>
An example graph: independent reviews and an adversarial test before the PR, CI before merge, and
every rejection back through repair. <a href="https://zeroshot.sh/#one-graph">More examples on zeroshot.sh</a>
</figcaption>
</figure>

## Start here

<!-- prettier-ignore-start -->

1. [Install](getting-started/install.md) the CLI:

    ```console
    npm install --global @the-open-engine-company/zeroshot
    ```

2. [Run a first task](getting-started/first-run.md) with the built-in `software-change` graph.
3. [Build a review loop](guides/review-loop.md): write your own graph and runtime plan.

<!-- prettier-ignore-end -->

Working in ChatGPT, Codex or Claude? The [plugin](guides/plugins.md) runs zeroshot without the
CLI. Prefer a browser? [`zeroshot ui`](getting-started/install.md#open-the-workspace-ui) edits
profiles, follows live runs and replays finished ones.

## What a run needs

- A **graph**: the steps, the routes between them and the typed state they share.
- A **runtime plan**: the agent CLI, provider and model for each step, and the connections it needs.
- An **input**: the task. zeroshot checks it against the graph before any agent starts.

Model output cannot add an edge or raise a bound. zeroshot records every step's result as the run
advances, and closing the terminal does not stop the run. [How execution works](concepts/execution.md)

## Built-in graphs

Run them as they are, or export one as the start of your own.

| Graph             | What it does                                                                                                              |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `software-change` | Implements the task, runs acceptance and code review in parallel, repairs what they reject, then delivers.                |
| `single-worker`   | One agent, no review loop.                                                                                                |
| `pr-review`       | Reviews a pull request.                                                                                                   |
| `code-review`     | Reviews a pull request in depth.                                                                                          |
| `auto-research`   | Runs ten rounds of experiments by default. A new result replaces the current one only when the progress judge accepts it. |

## Where it runs

| Target             | Where code and run state live                                                                                                                             |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Local              | Any folder on your machine (`--cwd FOLDER`). Git is optional without delivery. [Install](getting-started/install.md)                                      |
| Self-hosted target | Infrastructure you control. The target image checks out the source; agents and the run ledger stay there. [Targets](concepts/targets.md)                  |
| zeroshot Platform  | A managed target that takes the same graph and runtime plan, with accounts, organization policy and queues. [Platform docs](https://app.zeroshot.sh/docs) |

### Where code is processed

The target and the model provider are separate choices. Wherever a run happens, the agent sends its
model provider the code and context it needs, so self-hosting zeroshot does not make model
processing local. Check the provider's data-handling terms before running on client code.
[Runtimes and connections](concepts/runtimes-and-connections.md) covers providers and credentials;
for Platform, see the [privacy notice](https://app.zeroshot.sh/legal/privacy) and
[retention](https://app.zeroshot.sh/legal/privacy#retention).

## Reference

- [CLI](zeroshot-cli.md), generated from the command tree
- [Python API](reference/python.md), generated from the SDK
- [Cluster API](reference/cluster/api.md), generated from the OpenRPC contract

These pages follow `main` and may describe unreleased changes. For an installed release, pick its
version in the header. [Documentation versions](project/versioning.md)
{ .docs-note }
