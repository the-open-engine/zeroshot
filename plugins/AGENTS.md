# Shared skills and remote plugins

Edit canonical skills in `npm/zeroshot/skills`; don't fork instructions by client.
Both remote packages copy the run, plan and graph-design skills unchanged. Keep skills
self-contained and preserve npm installation, native self-update and the four skill release assets.

`scripts/plugins.js` owns `plugins/zeroshot-*` and both marketplace catalogs under
`.agents/plugins/` and `.claude-plugin/`; never edit generated files.
Increase the shared SemVer in `plugins/metadata.json` whenever emitted content changes.
It versions both client packages independently of runtime tags; runtime-only changes need no bump.

For package changes, run from the repository root with `PR_BASE_SHA` set to the fetched exact PR base:

```bash
npm run plugins:generate
npm run plugins:check -- --base "$PR_BASE_SHA"
npm run plugins:package
```

Use the [review cases](../docs/guides/plugins-review.md) for plugin releases; retain the tested
commit or ZIP checksum and observed client receipts. Keep credentials outside packages and chat.

## MCP and publication

Check runtime/workflow changes against the hosted MCP tools in zero-cloud. Coordinate skill and
tool changes across both repos; deploy required backend support before offering a package.
Keep published tool contracts and older cached skills working while replacements propagate.
GitHub installs cache versioned copies; merging doesn't update every installed client immediately.

- OpenAI package changes require a new ZIP, review and publication. Hosted tool metadata uses
  continuous review; compatible server fixes need no package release. Inspect live/held tool
  definitions after deployment. See [OpenAI's update process](https://developers.openai.com/plugins/deploy/submission).
- Claude's directory scans its tracked branch/tag. Each version must pass checks; publication
  follows the listing's reviewer/auto-publish setting. MCP connector listing edits are separate.
  See [Claude's update process](https://claude.com/docs/connectors/building/after-publishing).

Repository generation and CI don't publish directory updates. Verify current vendor requirements
before submission. User installation and updates belong in the [plugin guide](../docs/guides/plugins.md).
