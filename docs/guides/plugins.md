---
description: 'Install the Zeroshot plugin and authorize its MCP connection in ChatGPT, Codex, or Claude.'
---

# Install the Zeroshot plugin

The plugin includes the `zeroshot`, `zeroshot-plan`, and `zeroshot-graph-design` skills and a
connection to Zeroshot Platform. You need a Platform account; the Zeroshot CLI is optional.
Use the repository marketplace while public directory listings are pending.

## Codex CLI

```bash
codex plugin marketplace add the-open-engine/zeroshot
codex plugin add zeroshot@the-open-engine
```

Start a new Codex session and follow the plugin's connection prompt. Use `/plugins` to find the
installed Zeroshot plugin. The Codex IDE extension doesn't support plugins.

## ChatGPT and the desktop app

For a personal desktop install, add the marketplace with the Codex command above, restart the
ChatGPT desktop app, and open **Plugins > The Open Engine > Zeroshot** to install it.

For a workspace install, ask an admin to:

1. Open **Admin > Plugins > Add > Import marketplace**.
2. Set **Source** to `https://github.com/the-open-engine/zeroshot`, leave **Path** empty, import,
   and make Zeroshot available to members.

Then open **Plugins**, select your workspace, and install **Zeroshot**. Follow its connection
prompt. Repository imports can receive a **Desktop only** label even with a remote MCP server;
use the desktop app if that label appears. ChatGPT web needs a plugin available to that workspace
or account. See [OpenAI's workspace import instructions](https://learn.chatgpt.com/docs/enterprise/plugin-management).

## Claude Code

```bash
claude plugin marketplace add the-open-engine/zeroshot
claude plugin install zeroshot@the-open-engine
```

Start a new Claude Code session, enter `/mcp`, select the Zeroshot server, and authenticate.

## Claude chat and Cowork

1. Open **Customize > Plugins > Add > Add marketplace**.
2. Enter `the-open-engine/zeroshot`, then find **Zeroshot** and select **Add**.
3. Open the installed plugin's **Connectors** tab. Add the Zeroshot connector if it shows
   **Not added**, then connect it. On Team/Enterprise, an owner may need to add it first.

Account-installed plugins sync to Claude Code when you sign in with the same account;
CLI-only installs stay on that machine.
See [Claude's plugin instructions](https://claude.com/docs/plugins/overview).

## Authorize and check the connection

In the browser prompt, sign in to Platform, choose your organization, and approve access. The
plugin declares `https://api.app.zeroshot.sh/mcp`; you don't need to add a second MCP server.
Adding that URL alone gives you tools without the packaged skills.

Start a new chat and ask:

> Use Zeroshot MCP to list my runs. Don't start or stop anything.

You should get runs from the selected organization, or an empty list if it has none. Check
the installed plugin's details for the three skills named above. If skills appear but tools don't, finish
MCP authorization; if tools appear without skills, check that the plugin is installed and enabled.

Before running work, configure the required provider and repository connections in
[Platform Connections](https://app.zeroshot.sh/docs/connections). These are separate from signing
in to MCP. Enter credentials in Platform's browser forms, never in chat.

## Use it

| Task           | Example prompt                                                                                            |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| Plan           | “Use Zeroshot to plan this change from the attached files. Don't run it.”                                 |
| Design a graph | “Use Zeroshot graph design and save the agreed profile in my organization.”                               |
| Run            | “Use Zeroshot MCP to run org:default on OWNER/REPO, branch BRANCH, at commit FULL_SHA, with PR delivery.” |
| Inspect        | “Use Zeroshot to inspect this run and explain its current status.”                                        |

Supply repository files for planning in a browser. For a run, replace the source placeholders with
verified values and describe the change you want. Planning and saving profiles don't start runs.
Closing a chat or timing out leaves an accepted run running; ask explicitly to stop or resume it.

## Update

Refresh Codex's marketplace with `codex plugin marketplace upgrade the-open-engine`, then update
the plugin in `/plugins`. For Claude Code, run `claude plugin update zeroshot@the-open-engine`.
Claude chat/Cowork can check for updates from the plugin's marketplace. Start a new session afterward.

Existing CLI skills can coexist with the plugin. Explicit CLI requests, including
`zeroshot ... --target platform`, keep native CLI login. Stacks, ACP, and local Memory
initialization use the [CLI](../zeroshot-cli.md).

Maintainers: see [package generation and review cases](plugins-review.md).
