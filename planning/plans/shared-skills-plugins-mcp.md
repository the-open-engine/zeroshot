# Shared Zeroshot skills and MCP plugins

The OpenAI and Claude packages use the same canonical run, planning and graph-design skills.
They connect to the public Zeroshot MCP service; local CLI workflows remain available separately.

Public integrations use the server's advertised tool schemas and existing account permissions.
Keep exact source revisions, submission identity and observation cursors intact. Planning and
profile saves do not authorize execution, and credentials belong in browser setup rather than chat.

- [Installation and updates](../../docs/guides/plugins.md)
- [Generation, versioning and publication rules](../../plugins/AGENTS.md)
- [Client acceptance and directory review cases](../../docs/guides/plugins-review.md)

Package validation and local client smoke checks do not establish complete GUI workflow coverage
or public directory approval. Record those results separately when performed.
