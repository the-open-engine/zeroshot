# Shared Zeroshot skills and reviewed MCP plugins

Status: implementation and local verification are complete. Full GUI workflows and public
directory submission remain external release gates.

## Goal and boundaries

Users install Zeroshot in ChatGPT/Codex or Claude, authorize their Platform account, and plan work,
design/save a graph, or submit/follow a run without installing the Zeroshot CLI. Existing local CLI
workflows continue to work. Plugin sources and both marketplace catalogs live in
`the-open-engine/zeroshot`; MCP implementation stays in `the-open-engine/zero-cloud`.

Grounded in Zeroshot `86d9ffd67d3a0a8560f665c8d15497244288cff9` and Platform
`ce43e4a1c27b85956c65c906c7eada400a12a03c`, checked on 2026-11-09. Rebase and confirm these
contracts before implementation. Public directory acceptance remains a vendor decision; the release
checklist below includes that external milestone rather than treating a local validator as approval.

Keep one `/mcp` endpoint, its OAuth bridge, and existing native handlers. Replace the unreleased
five-tool catalog in place. Do not add aliases, a skills-serving MCP API, another repository, a GitHub
browser integration, runtime changes, a plugin installer, lifecycle hooks, or a new publishing service.
Stacks and local history initialization stay outside the remote plugin scope.

## Shared skills and packages

Keep `npm/zeroshot/skills/` as the only authored skill source, as required by `AGENTS.md`.
Update `zeroshot`, `zeroshot-plan`, and `zeroshot-graph-design` in place with short inline CLI/MCP
branches; keep each `SKILL.md` self-contained. Leave `zeroshot-memory-init` available through existing
CLI distribution and omit it from remote plugin packages. This preserves the npm installer, native
self-update, managed-copy conflict handling, opt-out behavior, and four immutable single-file skill
assets without expanding their formats.

Keep target and transport separate. Explicit MCP/plugin requests require the connected Zeroshot
server and lead to setup when disconnected, without silent CLI/local fallback. Explicit CLI requests,
including `--target platform`, retain native CLI login/access without needing MCP. Otherwise use
available access for the user's chosen target; never turn remote work into local execution.
Refer to logical tool names, not host-generated `mcp__...` prefixes, and test coexistence with
npm-installed user-scope skills.

The shared MCP branch must cover:

- **Run:** inspect the profile and its delivery behavior, use supplied repository/branch/exact revision,
  confirm unresolved source or delivery choices, and submit only after the user requests execution.
  Retain the submitted payload and submission key for identical retries. Submit accepts an omitted
  run ID; a lost response must recover the original receipt through native replay without requiring
  the client to generate UUIDv7. Resume separately retains its explicit successor ID and checkpoint.
  Follow cursors through status/logs; wait timeout or chat disconnect leaves execution running.
  Stop, discard, and merge follow the user's intent. Report observed completion and delivery receipts.
- **Plan and graph design:** use repository evidence available through the host, attachments, or the
  user; a browser chat has no implied checkout. Ask for missing evidence without inventing repository
  facts. Return portable plan text/artifacts when no writable checkout exists. Graph authoring uses
  the server's catalog/templates and validator; save through MCP, read back, and retain the agreed
  node instructions and caller-selected runtime identifiers. Saving or planning does not start a run.

Provider credentials are configured in Platform through browser setup links. Never ask for secrets
in chat. Missing context may limit a draft; unresolved evidence needed for safe execution blocks
submission, rather than becoming a claim that the repository was inspected. Local-only memory and
stack instructions must remain explicitly confined to their supported CLI paths.

Add `scripts/plugins.js` with generate, check, and package modes. One small
`plugins/metadata.json` supplies shared identity, plugin content version, publisher links, and assets.
Generate and commit regular files in both package folders; do not use symlinks or references outside
the folder. Package generation copies canonical skill bytes unchanged.

```text
npm/zeroshot/skills/                     canonical authored skills
plugins/metadata.json                   shared plugin metadata and content version
plugins/zeroshot-openai/                 .codex-plugin/plugin.json, .mcp.json, skills/, README, LICENSE, assets/
plugins/zeroshot-claude/                 .claude-plugin/plugin.json, .mcp.json, skills/, README, LICENSE, assets/
.agents/plugins/marketplace.json         OpenAI catalog
.claude-plugin/marketplace.json          Claude catalog
scripts/plugins.js                      deterministic generation, drift check and ZIP packaging
tests/tooling/plugins.test.js            package contracts
```

Use stable plugin name `zeroshot` and marketplace name `the-open-engine`. Each catalog lists only
its corresponding package. Both packages point to `https://api.app.zeroshot.sh/mcp` with `type: http`.
The OpenAI package uses the supported Codex manifest with explicit skills and MCP file mappings,
verified against the installed client's loader. Include valid listing metadata, actual support and
privacy links, an icon, license, and an installation README in each package. Keep package contents
to Markdown, JSON, and assets: no dependency installation, `bin/`, hooks, app-ID mappings, or secrets.
Check both source directories and the extracted ZIP independently of the repository.
The OpenAI server entry declares `extensions.com.openai.auth` with `type: oauth` and
`client.mode: dcr`; use the existing discovery/registration flow and advertised scopes. Claude uses
its normal OAuth challenge/discovery flow. Neither package embeds API keys or claims unauthenticated access.

Use one valid release SemVer for both packages. When emitted non-version content changes, require
a strictly greater version than the PR base; first introduction has no previous version. Ignore only
the generated manifests' own version fields in this comparison; version-only increases may pass.
This version supports client update detection; it introduces no
new runtime release tags/workflow or Cargo/npm version staging. Leave canonical native releases,
their updater assets, and recovery checks unchanged. Document this ownership in `AGENTS.md`.

## MCP changes for public review

Keep a closed registry that owns each tool's name, title, description, typed input decoder/schema,
annotations, and fixed native operation. Remove the separate hardcoded name list. Deserialize with
the same Rust DTO used to produce the schema; reject unknown envelope fields. Reuse existing
native graph/runtime/environment types and their schemas instead of maintaining another domain
schema. No caller-supplied method, route, or generic native `request` object remains.

Retain all current nonsecret operations, one tool per operation: **31 tools**. Remove
`connections/set`; credentials remain configurable through the existing native/UI path.

| Domain       | Tools                                                                                                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Runs         | `list_runs`, `get_run`, `submit_run`, `submit_profile_run`, `wait_for_run`, `read_run_logs`, `stop_run`, `list_run_checkpoints`, `resume_run`, `discard_run_workspace` |
| Profiles     | `list_profiles`, `get_profile`, `save_profile`, `delete_profile`, `set_default_profile`, `get_authoring_catalog`, `validate_profile`                                   |
| Environments | `list_environments`, `get_environment`, `save_environment`, `delete_environment`, `list_environment_repositories`                                                      |
| Connections  | `list_connections`, `delete_connection`, `check_connection`, `start_github_connection`, `start_github_installation`                                                    |
| Memory       | `list_memory_lessons`, `get_memory_lesson`, `update_memory_lesson`, `delete_memory_lesson`                                                                             |

Use secret-free MCP DTO projections for submit, profile-run, and resume. Hosted submit requires
`ResolvedSource` in its schema even though native local `RunSubmission.source` is optional. Preserve
optional server-generated run IDs for both submission forms. Construct native requests server-side
with empty inline credential maps and absent `githubToken`, `gitCredentials`, and
`connectionResolver`. Profile persistence must expose only its native nonsecret fields.
Preserve named connection requirements in runtime nodes and environments;
those declare field names, not secret values. Do not recursively ban every `connections` key or scan
opaque task/initial-input strings for secrets. Test actual excluded envelope fields and unknown fields.

Retain concrete `environment` and separate `environmentRef` on both submit forms; preserve environment
`egress`/`defaultFor`, optional `id`, save's required-nullable `expectedRevision`, and delete's required
revision. Preserve profile `setDefault`, nullable default-profile name clearing, checkpoint `after`/`limit`
versus history/wait cursors, and nonsecret commit identity where supported.
Environment/Memory writes carry captured `workspace_id` to the existing native header and revision
checks. Omitted scope defaults to user for humans/personal machines and org for organization machines;
Memory always uses org scope.
Keep the existing native profile endpoints rather than moving operations to a different persistence API.

Every tool gets a title plus explicit `readOnlyHint`, `destructiveHint`, and `openWorldHint` booleans.
Classify behavior, not the internal HTTP verb: profile validation and checkpoint listing use
POST internally but do not mutate domain state. Profile list/show can initialize or refresh native
defaults, and repository listing can refresh stored GitHub credentials; disclose those effects and
mark these operations non-read-only/destructive. Submit/resume can publish or merge, so mark them
destructive and open-world. Mark upserts/default changes, updates, deletes, cancellation and discard
as destructive. `check_connection` can persist refreshed credentials, so mark it non-read-only and
destructive; disclose that effect. OAuth setup starts are non-read-only, additive setup-state writes.
Omit optional idempotency hints unless demonstrated for that specific operation. Bounded private
catalog reads are not open-world merely because they use a remote server.

Keep the existing native authorization, organization/machine ownership, human-only Memory,
workspace identity and expected revision checks. Preserve bounded wait (maximum 30 seconds), history
cursors/availability, structured native results/problems, body limits, and retry semantics. Keep the
current DCR/PKCE/refresh/revocation flow and API/app origin separation; do not claim narrower OAuth
grants than the bridge actually issues. Update initialization text to describe the new catalog.

Primary Platform files: `backend/services/api-gateway/src/billing/public_api/mcp.rs`,
`mcp/catalog.rs`, `mcp/operations.rs`, the owning native DTO/schema definitions only as needed,
`backend/services/api-gateway/tests/billing_http_interface/mcp.rs`, `tests/e2e/mcp/`,
`frontend/components/docs/docs-mcp.tsx`, `docs/mcp.md`, and `AGENT.md`.

## Implementation order and acceptance

Implement as two dependent code changes: Platform's reviewed MCP contract first, then Zeroshot's
shared skills/packages. Use the tool names above on both sides. Update existing MCP
fixtures, scripts, docs, and hardcoded five-tool expectations in the Platform change; seed provider
credentials through fixture/native setup rather than the removed MCP operation. Use the same
endpoint rather than adding a compatibility endpoint.

In Zeroshot, add generation/drift/version tests to the existing fast contract-test command in
`.github/workflows/ci.yml`, so every PR runs them without another job, and retain full tooling coverage. Extend
`.github/ci-path-classifier.js` and its tests so all three changed canonical skills select npm and
tooling, and package/catalog/metadata edits select tooling.
The fast checkout must fetch and pass the event's exact base SHA for version comparison; missing
history fails that gate. First introduction applies only when the fetched base proves no prior metadata.
Add one `docs/guides/plugins.md` entry to `mkdocs.yml`; document install, authorize, upgrade, supported
workflows, and CLI coexistence. Platform's existing served MCP guide links that guide and shows the
new tool names. An additional landing-page project is unnecessary.

| Acceptance                               | Required evidence                                                                                                                                                                                                                                                                                                                                                                                 |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| AC-1 Shared source                       | Both packages contain exactly the three supported skills, byte-identical to canonical source. Drift, missing/extra files, escaping paths, symlinks, and leaked private fixtures fail the package check.                                                                                                                                                                                           |
| AC-2 Distribution and upgrade            | Extract/install each package by itself. Catalog paths, identity, endpoint and version agree. Unchanged/decreased versions with changed payload fail; installing an increased version exposes changed instructions. Run `claude plugin validate --strict --profile directory` on its package and strict validation on its marketplace; validate the OpenAI schema/package and install it in Codex. |
| AC-3 Existing CLI behavior               | Existing npm skill-install/opt-out/conflict tests, package-install tests and distribution checksum tests pass. Local requests and native CLI `--target platform` work with no MCP configured, including when plugin/user-scope copies coexist.                                                                                                                                                    |
| AC-4 Reviewed tool contract              | Registry/schema tests prove exactly 31 independently callable tools with strict inputs, correct titles/annotations, and no legacy executor or secret-writing tool. Native/MCP parity covers each retained route; POST-only reads and token-refreshing checks receive their correct classifications.                                                                                               |
| AC-5 Credential and authority boundaries | Submit/profile-run/resume reject raw credential envelopes; named references still resolve server-side. Test required source, all nondefault fields above, malformed/unknown inputs, cross-owner access, member/admin writes, actor-derived scopes, machine refusal on every Memory tool, stale revisions/workspace identity, and secret-free output/errors.                                       |
| AC-6 Retry and observation               | Existing MCP interface/e2e fixtures prove lost-response retry with omitted run ID returns one original run, changed payload under the same key conflicts, resume preserves its explicit successor/checkpoint, cursor paging is continuous, wait is bounded, and timeout/disconnect does not stop the run.                                                                                         |
| AC-7 Installed clients                   | Fresh installs in Codex CLI/app, ChatGPT, Claude Code, and Claude chat/Cowork discover the packaged skills. Browser workflows run with no CLI or local checkout. Verify OAuth consent/refresh/revoke, reconnect after expiry, and explicit setup when Platform connections are missing. Record supported surfaces and minimum tested client versions.                                             |
| AC-8 Complete workflows                  | Run the complete plan, graph/save/readback, and authorized change/delivery workflows in ChatGPT browser, Codex CLI, Claude chat browser, and Claude Code. Include failed-run and missing-evidence cases. AC-7 discovery/auth smoke checks cover remaining documented app/Cowork surfaces. Preserve redacted transcripts and verify results independently in Platform.                             |
| AC-9 Submission package                  | All final files pass vendor preflight; exact review ZIP/commit, tested cases, recording, metadata, domain verification and reviewer-access instructions are ready. Credentials stay outside source/ZIP. Portal findings must be resolved before declaring ready for review.                                                                                                                       |

Automated gates: focused Node tooling/package/CI-classifier tests, `npm run lint`, `npm test`,
`npm run distribution:check`, and strict documentation build in Zeroshot; focused Rust MCP/interface
tests plus `npm run e2e:mcp`, `npm run e2e:mcp:browser`, and affected frontend checks in Platform.
Run each repository's required Opcore checks for implementation changes and the applicable full CI
lanes. Installed GUI-client and public-portal checks are operator gates, recorded as unverified until
performed; package parsing alone cannot satisfy AC-7 through AC-9.

## Implementation verification (2026-11-09)

Zeroshot branch: `codex/shared-skills-plugins`. Platform branch: `codex/reviewed-mcp-tools`.
The verification below records the implementation before release.

| Check                    | Observed result                                                                                                                                                                                                                                                                                                                  |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Zeroshot tooling         | 149 tests passed; lint, Rust formatting, distribution checks, exact-base plugin/version checks, and changed-file formatting passed. Existing installer/update protections remain covered.                                                                                                                                        |
| Skill and package checks | Three canonical skill validators passed. Both ZIPs are standalone, contain the same canonical skills, and pass drift/path/version checks. Claude package and marketplace strict validation passed.                                                                                                                               |
| Installed clients        | Codex CLI 0.167.0 loaded all three skills and bundled MCP; a temporary 0.1.0 → 0.1.1 upgrade exposed changed skill metadata. Claude Code 2.1.317 installed the package and reported exactly three skills and one MCP server. Task installs were isolated and removed.                                                            |
| Documentation            | Strict MkDocs build passed using CI's Python 3.12 and the exact `docs/requirements.lock`, including the local Python SDK.                                                                                                                                                                                                        |
| Platform static checks   | Frontend lint/typecheck and workspace/all-target/all-feature backend formatting/Clippy with warnings denied passed.                                                                                                                                                                                                              |
| Platform tests           | 1,119 frontend unit tests, 577 frontend integration tests, 17 focused MCP unit tests, and two database-backed MCP HTTP tests passed. The broader API library run passed 589 tests with one existing S3/MinIO fixture test explicitly ignored.                                                                                    |
| SDK acceptance           | Passed against the final production-feature backend binary in the isolated Docker stack: OAuth/PKCE/refusals, all 31 tools, resource/revision/secret checks, lost-receipt retry, changed-payload conflict, a successful hosted run, history cursors, bounded wait, all four machine Memory refusals, refresh/reconnect/revoke.   |
| Browser acceptance       | Passed real frontend sign-in and organization consent, OAuth callback/code exchange, host-bound secure session cookie, canonical API resource enforcement, 31-tool discovery and profile setup. Zero browser bearer-token leaks observed.                                                                                        |
| Independent review       | Separate agents challenged the skills, package generator, MCP mapping/authority/effects, retry evidence, and final SDK schema compatibility fix. Reported issues were fixed and re-reviewed. A fresh skill-only review covered CLI/browser routing, missing evidence, graph save and lost receipts without executing operations. |
| Opcore                   | Post-edit gates accepted; Verify clean. Sense reports partial existing dependency-resolution coverage, so it is not represented as a complete dependency analysis.                                                                                                                                                               |

The SDK found a boolean JSON Schema shorthand rejected by its catalog parser. The adapter uses
Schemars' existing compatibility transform; regressions prove schema constraints and literal
defaults/enum/constants/examples remain intact. Native Claude testing also exposed required fields
whose schemas incorrectly excluded null. Both now retain requiredness and native nullability.
Opaque initial input explicitly advertises every JSON type so Claude can send a JSON null without
substituting a string. Decoding remains unchanged, with regressions preserving all input values,
including the literal string `"null"`.

The final production-feature test executable has SHA-256
`879c1658f4777e36c8bee43127122a8a394013fcefa29059bc0544634cd19030`.

The installed Codex plugin also completed real OAuth/DCR, browser consent and authenticated
discovery of exactly 31 tools, with TLS verification enabled and an isolated ephemeral keyring.
Its test copy differed only in the local MCP endpoint. This required a proper server certificate
signed by the test CA; no product workaround or host trust change was needed.

Final version 0.1.0 ZIP SHA-256 checksums:

- OpenAI: `1cab3865a08242f2bf83b2d2b8de434c06e9e37372c298ada9941eec6872cb0e`
- Claude: `15424341d6244ce66e2a81d30d72a84cb1b5fb952c70c18ba693d1e2adfd3a8c`

### Native Claude CLI against the full local stack

Claude Code 2.1.317 installed the package from a temporary marketplace, with only its MCP URL
redirected to the local HTTPS stack. Its browser OAuth flow and all 31 bundled tools worked.
Model sessions invoked each packaged skill: planning and graph drafting used supplied evidence
without claiming a checkout or making changes. Read checks covered runs, environments, connections,
and Memory. A profile/environment roundtrip checked validation, readback and stale-revision refusal.

Claude then submitted one hosted terminal-fixture run, replayed the identical submission, followed
its successful status/history, and deleted the test resources. Independent database/archive checks
confirmed one intent, exact source and JSON-null input, the environment witness, and complete
terminal history. Existing profiles, defaults and revisions matched their pre-test baseline.
With the plugin disabled, a separately installed native MCP connection exposed 31 tools and read
back that same result using only its own tools. The final SDK regression also passed after both
schema fixes. No string coercion or direct HTTP fallback was used by the model.

This run had no agent steps or Git delivery. It verifies local CLI integration and runtime
submission/supervision; it does not satisfy the full software-change/PR workflow in AC-8.
Private transcripts, failed-attempt diagnoses, independent assertions and cleanup reports are under
`/home/ec2-user/.cache/claude-full-stack-Vq23QK5X`.

Local redacted logs and private fixture outputs are under
`/home/ec2-user/.cache/shared-skills-plugin-checks`; credentials and fixture identities are not
included in either package. Automated authentication checks do not stand in for the complete
ChatGPT/Claude GUI workflows in AC-7/AC-8. Those workflows, the populated public reviewer account,
walkthrough, portal/domain verification, deployment, and directory submission/publication remain
unperformed operator gates under AC-7 through AC-9.

## Release and offering

Deploy the verified Platform adapter before offering packages that call the new tools. Ship both
marketplace catalogs from the public Zeroshot repository and offer install instructions immediately;
public listings are a separate release milestone. No automatic external publication runs in CI.

For OpenAI, submit the complete remote-MCP-plus-skills ZIP from a verified publisher, complete the
domain challenge, and provide exactly five positive and three negative tested cases plus a walkthrough.
Use these cases across client testing: inspect profile; plan only; validate/save graph; submit/follow
work; inspect/recover an authorized run. Negative cases cover unrequested execution/destruction,
missing source/setup, and insufficient authority. Each record has its concrete prompt, fixture/exact
source, expected tools/result, and forbidden-action assertions; use its ID in transcripts and video.
Preconfigure a populated reviewer account, named provider connections and disposable repository.
Prove access without private networks, MFA, email/SMS verification codes, magic links, or team
assistance; an incompatible login path blocks submission.

Obtain the portal's domain token and select an allowed Platform parent HTTPS origin. Serve only
that exact plain-text token from `frontend/public/.well-known/openai-apps-challenge` through the
existing Platform frontend deployment, with no browser session required, then verify it externally
and in the portal. Use the API origin instead only if the portal requires it; that requires an exact
static response/edge route, not another verification service. Confirm all listing/legal/support URLs.

For Anthropic, submit the remote MCP connector and Claude plugin folder from the same owning
organization and pair them using the identical endpoint. Confirm the public GitHub repository and
final whole-repository archive/folder limits; include README, license, icon, privacy and support
metadata. Test the connector through the installed package's connect/authorize path. Track the exact
reviewed source revision and last published version. Failed/held updates must leave the published
package and live MCP contract usable.

Public release is complete only when both approved listings are published and their install links
work in the documented clients. Until then, label directory status accurately and use the repository
marketplace instructions. Later skill/metadata changes increment the shared plugin version and run
the same gates; compatible backend fixes deploy through the existing Platform process. Changes to
tool schemas must stay compatible with the definition currently approved by each directory until its
replacement becomes live.

## Vendor contracts

- [OpenAI packaging](https://developers.openai.com/plugins/build/plugins),
  [tool and credential requirements](https://developers.openai.com/plugins/plugin-guidelines),
  [submission and updates](https://developers.openai.com/plugins/deploy/submission).
- [Claude package format](https://claude.com/docs/plugins/build),
  [package checks](https://claude.com/docs/plugins/pre-submission-checklist),
  [MCP review criteria](https://claude.com/docs/connectors/building/review-criteria),
  [paired directory submissions](https://claude.com/docs/directory/publish),
  [version publication](https://claude.com/docs/plugins/submit).
