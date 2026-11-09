# Zeroshot

Plan software changes, design workflows, and run verified work on Zeroshot Platform.

This package contains the shared run, planning, and graph-design skills and the hosted
Zeroshot MCP connection. The Zeroshot CLI is optional for these Platform workflows.

## Install and connect

```sh
codex plugin marketplace add the-open-engine/zeroshot
codex plugin add zeroshot@the-open-engine
```

Start a new session after installation and authorize the Zeroshot MCP connection.
ChatGPT testing uses a local or workspace plugin source; public directory listings
require separate approval.

Ask to plan a change, design a reusable workflow, or run an existing Platform profile.
Execution requires your request and an exact repository commit. Configure provider
credentials in Platform; never paste secrets into a conversation. Local history
initialization and stacks require the CLI and are outside this package's MCP scope.

See the [installation and upgrade guide](https://the-open-engine.github.io/zeroshot/current/guides/plugins/) for client setup.
The package connects to https://api.app.zeroshot.sh/mcp; Platform processes the task data sent through
that connection under its [privacy notice](https://app.zeroshot.sh/legal/privacy) and
[terms](https://app.zeroshot.sh/legal/terms). [Support](https://github.com/the-open-engine/zeroshot/issues).

Generated from the canonical skills in this repository. Edit their sources rather
than these generated copies. Update the shared plugin version when package content changes.
