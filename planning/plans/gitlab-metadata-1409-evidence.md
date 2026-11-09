# GitLab metadata regression validation — issue #1409

Executed on 2026-11-09 against the submitted checkout at base
`86d9ffd67d3a0a8560f665c8d15497244288cff9` plus this patch. This record covers the review follow-up
that generated **repair commit messages**, as well as MR titles, must be normalized.
GitLab's [push refresh implementation](https://gitlab.com/gitlab-org/gitlab/-/blob/v19.5.1-ee/app/services/merge_requests/refresh_service.rb)
can mark an existing MR draft from a newly pushed commit message.

## Candidate and preparation

Candidate: `/var/lib/zeroshot/native-v2/runs/921d68e30c4ee4dc9f7876daf7034f0f2c9844dd026ed148c20680448cabe39f/tools/repair-1409/zeroshot-candidate`.
SHA-256: `6d641b9c0153188e0e9eb16237f7d5fb5335589e2f16151b52512312dfa445d0`.
The validation launcher links this build's `zeroshot_engine` library and executes this frozen
candidate as its controller. Subsequent source edits added tests and this record only.

The root and UI lockfile dependencies were installed with `npm ci --no-audit --no-fund`
and `npm ci --prefix ui --no-audit --no-fund`. Python dependencies were installed into the
repository `.venv` with `.venv/bin/python -m pip install -e 'sdks/python[dev]' -r docs/requirements.lock`;
`.venv/bin/python -m pip check` passed. `cargo fetch --locked` completed.
Manifests and lockfiles are unchanged; these ignored dependencies remain prepared for reviewers.

The prepared fixture at `$ZEROSHOT_TOOLS/fixtures/zeroshot-matrix` was available, and its recorded
revision, HEAD and supplied revision all matched `998c66fa78a9ee21a1b79466f7dc7d1c53cf47cf` before builds
and validation. Every scenario used a separate repository copy and private CLI state; the prepared
fixture was not modified.

```sh
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  cargo build --locked -p zeroshot --bin zeroshot
VALIDATION_RUN_CLI="$ZEROSHOT_TOOLS/repair-1409/cli" .venv/bin/python \
  "$ZEROSHOT_TOOLS/repair-1409/live.py" "$ZEROSHOT_TOOLS/repair-1409/zeroshot-candidate"
```

The supplied OAuth credential was provided only through the declared
`gitlab-validation-1409-20261109` connection's `GITLAB_TOKEN` field. Git pushes used a separate
private declared Git transport connection. Connection values were supplied on private stdin;
no credentials were placed in arguments, URLs, metadata, reports or Git history.

Platform credential egress requires its existing `/usr/bin/bash` wrapper. The test launcher uses
public `LocalCliBackend` to select a controller shim through that wrapper, restoring the Platform
proxy for the detached controller. It then executes the frozen candidate's
`__zeroshot-run-controller`. CLI parsing, graph execution, Git push, trusted delivery and the
GitLab adapter are the candidate implementations. No GitLab server or delivery adapter is mocked.

## Live execution and stored results

`GET /api/v4/version` returned **19.6.0-pre**, revision
`19ecfd260d5`. `GET /api/v4/user` confirmed
`michael.eichelbeck` / `15584353`.
New private disposable project: [michael.eichelbeck/zs-1409-repair-3332eb9262](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262),
ID `88709148`. No pre-existing user project was modified.

Each case ran this command with its own graph, runtime, input, repository and state directory:

```sh
$ZEROSHOT_TOOLS/repair-1409/cli run --title 'GitLab 1409 CASE' \
  --graph CASE/graph.json --runtime-config CASE/runtime.json --input CASE/input.json \
  --cwd CASE/repo --source-connection gitlab-validation-transport-1409 \
  --source-provider source.gitlab --source-base-url https://gitlab.com
```

The deterministic custom graph has three rounds that revisit one `builtin.git-delivery.pr@2`
node with exact script-authored metadata. Round one creates a commit, pushes and opens the MR.
After creation, the harness adds human text outside the generated markers. Round two changes
`metadata-validation.txt`, appends `Updated delivery report.` to the generated body, and uses
trusted delivery to commit, push and update the **same MR**. Round three retries without changes.
The CLI must finish with a succeeded terminal result and a `ready` delivery receipt.

| Authored title | MR | Stored title and repair commit message | Unicode characters | Draft / state after push processing | Run ID |
| --- | --- | --- | --- | ---: | --- | --- |
| `Draft: Ready title` | [!1](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/1) | `Ready title` | 11 | false / opened | `01a1c0db-141a-72a2-a1e4-21af733ac361` |
| `fix: publish literal metadata` | [!2](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/2) | `fix: publish literal metadata` | 29 | false / opened | `01a1c0dc-bb6f-74b1-87d4-6ee49543e228` |
| `a` × 300 | [!3](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/3) | `a` × 255 | 255 | false / opened | `01a1c0de-0395-7143-97f5-f7d043f3bdea` |
| `界🦀` × 150 | [!4](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/4) | first 255 characters of `界🦀` × 150 | 255 | false / opened | `01a1c0df-a11e-7c60-ad68-2880230891eb` |
| `[Draft] Ready title` | [!5](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/5) | `Ready title` | 11 | false / opened | `01a1c0e1-41b4-7490-a018-f709b6f1c89c` |
| `(Draft) Ready title` | [!6](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/6) | `Ready title` | 11 | false / opened | `01a1c0e3-008d-7233-814c-1c368946dcf4` |
| `Draft:` | [!7](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/7) | `Zeroshot change` | 15 | false / opened | `01a1c0e4-583b-7a73-8b0d-6787ed703930` |
| `[Draft]` | [!8](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/8) | `Zeroshot change` | 15 | false / opened | `01a1c0e5-e936-7590-ba13-7dea21d7066d` |
| `(Draft)` | [!9](https://gitlab.com/michael.eichelbeck/zs-1409-repair-3332eb9262/-/merge_requests/9) | `Zeroshot change` | 15 | false / opened | `01a1c0e7-8a66-7993-bd53-2257c601c718` |

Every row below has a new repair SHA with the creation SHA as parent. The API's latest MR diff
version identifies the repair SHA, and a GitLab system note records the added commit. Only after
both checks did the harness sample metadata four more times, one second apart; all samples were
nondraft and open. The API commit's complete message (apart from its final newline) equals the
canonical title. This checks server push processing, not merely the immediate metadata response.

| MR  | Source branch                      | Creation SHA                               | Repair SHA                                 | Recorded diff versions |
| --- | ---------------------------------- | ------------------------------------------ | ------------------------------------------ | ---------------------: |
| !1  | `zeroshot/v2-f8fa04a15bf3876d5a3f` | `0db5f4e674c6bc382be8aee052e23f62e13ab06e` | `e5d823625427e5dfafa0e50e1c896e0b70b63f2d` |                      2 |
| !2  | `zeroshot/v2-5d6b59df2daae9c47842` | `1dde7a24c984f93d5c0b8e47e3766985bb8e23fe` | `f079cabde9ad9cb9675e1f6415a2df6841663a3c` |                      2 |
| !3  | `zeroshot/v2-1a29e851eef5b5962d92` | `94ca9e8dcfffa1d03600ec89f39bf4cb03e49a9f` | `8d8577e2e1e2ac20480fad44423fbe1dab824eef` |                      2 |
| !4  | `zeroshot/v2-d7a4b423a2bf3004e665` | `13eb5ce2145c3ff16938b8f8fc992bf4ad241f64` | `5bf1b8ae223152690abe6a7d4b5647b625066f58` |                      2 |
| !5  | `zeroshot/v2-5a22ffa20e494bf7f5d1` | `3408345dee33d0d2cb0ca127ff1d88b554cabb8c` | `7b9b0a027f29f7cd55e953cae00550b82194d423` |                      2 |
| !6  | `zeroshot/v2-c52df77ef7c68ab822cd` | `e9c519e855f0e76eb216dcca49afd2dc2f76e8ba` | `c9cc572973265c1a57f07f7f8b3ba5ee528edc08` |                      2 |
| !7  | `zeroshot/v2-1807675d1e3bbec01411` | `eaac8ca4f201f47680c5acfe4029881039d4cc8f` | `b37b88644e9afe35559778dae6d5481ed99cc2fe` |                      2 |
| !8  | `zeroshot/v2-1bb95252455b3d6e0deb` | `02879d86fa31dddc167481b1b0af0378c89164f3` | `5577d1a235713b37404fd96f06733f5823075862` |                      2 |
| !9  | `zeroshot/v2-9322f5fe64fe69fb7ff2` | `88cc268908ecec30179d08dc102aac2804b1372e` | `e7c84c20499c62ff5ec52043d902118eb21be441` |                      2 |

All generated bodies contained standalone `/health returns 200` and `/close`, an already escaped
`\/close`, inline code, backtick and tilde fenced code, and an indented example. Stored slash-leading
lines had exactly one backslash; existing escapes stayed unchanged across update and retry.
The GitLab Markdown API confirmed readable literal endpoint and quick-action-looking prose.
MRs stayed open and system notes contained no close/reopen event. The live 300-character ASCII
and multibyte cases retained their complete original title exactly once in the generated section;
all prefix-only forms became `Zeroshot change`.

The human prefix and suffix were preserved exactly. Final retry descriptions and SHAs equaled
the preceding update; no normalization note or escape was added again, and the branch API head
matched the final delivery receipt. For !1, the actual stored updated description was:

````text
Human preface.

<!-- zeroshot-delivery:generated:v1:start -->
## Summary
\/health returns 200
\/close
\/close
`/health`
```text
\/close
\/health returns 200
```
~~~text
\/close
~~~

    \/close

Existing escaped endpoint: \/health.
Updated delivery report.
<!-- zeroshot-delivery:generated:v1:end -->

Human follow-up.
````

## Changed paths

- `zeroshot/src/source_code/adapters/core/gitlab/`: literal body formatting, shared title normalization, MR refresh and regression tests.
- `zeroshot/src/source_code/provider/delivery.rs` and `zeroshot/src/native_v2_delivery/`: the pure commit-message hook, its call before preflight/commit creation and the existing-MR repair regression.
- `zeroshot/src/native_v2_comment_delivery/tests.rs`: canonical publication hashes and lost-response behavior.
- `zeroshot/src/native_v2_templates.rs`, `docs/guides/forge-delivery.md` and `AGENTS.md`: metadata guidance and adapter ownership.
- `planning/plans/gitlab-metadata-1409-evidence.md`: this validation record.

## Local checks

Focused commands used `--locked`, `CARGO_BUILD_JOBS=1`, debug information disabled, and
`CARGO_INCREMENTAL=0`. Rust tests used `--test-threads=2` (the single regression used one).

| Command                                                                                       | Observed result                                                                                                           |
| --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `cargo build --locked -p zeroshot --bin zeroshot`                                             | exit 0; frozen candidate above                                                                                            |
| `cargo test --locked -p zeroshot --lib gitlab -- --test-threads=2`                            | 71 passed; bounded/redacted API diagnostic tests included                                                                 |
| `cargo test --locked -p zeroshot --lib gitlab_normalizes_commit_messages -- --test-threads=1` | 1 passed; three actual local Git pushes with canonical subjects                                                           |
| `cargo test --locked -p zeroshot --lib native_v2_delivery:: -- --test-threads=2`              | 214 passed, 1 existing opt-in campaign ignored                                                                            |
| `cargo test --locked -p zeroshot --lib native_v2_comment_delivery:: -- --test-threads=2`      | 46 passed; publication hash/intent and lost-response checks included                                                      |
| `cargo test --locked -p zeroshot --lib native_v2_cli::templates:: -- --test-threads=2`        | 39 passed                                                                                                                 |
| `cargo test --locked --workspace -- --test-threads=2`                                         | exit 0; complete workspace, integration and doc tests passed; main library: 2417 passed, 13 existing opt-in tests ignored |
| `cargo clippy --locked --workspace --all-targets -- -D warnings`                              | exit 0                                                                                                                    |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --no-deps`                            | exit 0                                                                                                                    |
| `npm run check`                                                                               | exit 0; lint, 137 tooling tests, distribution and generated protocol checks passed                                        |
| `.venv/bin/python -m mkdocs build --strict`                                                   | exit 0                                                                                                                    |
| `.venv/bin/python -m pip check`                                                               | no broken requirements                                                                                                    |
| `cargo fmt --all -- --check`                                                                  | exit 0                                                                                                                    |

An initial native-test compilation used an incorrect import in the newly added test; it was
corrected. The passing regression and 214-test delivery rerun above supersede `delivery-tests.log`.
Production diagnostics were not changed to accommodate any preparation failure.

Repository-wide `npm run format:check` exited 1 on 23 pre-existing files outside this patch.
Every warned file was compared byte-for-byte with `HEAD` and matched (`format-baseline.json`).
Changed Markdown files pass the focused Prettier check. Unrelated formatting is not part of this patch.

Local Unix subprocess tests use a real private executable `/var/tmp` directory. Their loopback-only
PATH selects the original bash/GitHub CLI tools and a Git wrapper that suppresses the Platform's
system proxy only when the test has not authored `GIT_CONFIG_SYSTEM`. This prevents Platform
credential egress from intercepting synthetic fixture hosts. Live GitLab uses the Platform proxy.
These are validation preparation changes outside the product checkout.

## Cleanup and independent evidence

Current live result: **passed**. Cleanup: **project marked for deletion**.
Private connection state is removed after each scenario and again in cleanup. GitLab scheduled
project deletion is its normal accepted deletion result; references above are intentionally disposable.

Cleanup API readback:

```json
{
  "id": 88709148,
  "path_with_namespace": "michael.eichelbeck/zs-1409-repair-3332eb9262-deletion_scheduled-88709148",
  "visibility": "private",
  "marked_for_deletion_on": "2026-11-09",
  "marked_for_deletion_at": "2026-11-09",
  "pipeline_count": 0,
  "local_secret_state_removed": true
}
```

Full sanitized commands, API reads, stored metadata, diff versions, push notes, branch reads and
terminal receipts are retained at `$ZEROSHOT_TOOLS/repair-1409/live-evidence.json`; the harness,
graphs, inputs and native logs remain alongside it. Build provenance is `build-evidence.json`.

The preceding independent behavior review accepted its candidate
`ef6da50422e10cd72447fcaccb494bbb7b147963f8a1ed8458c26d5b754bcd71`, including ten live scenarios
in [project 88708143](https://gitlab.com/michael.eichelbeck/zs-1409-v-71e061dade).
Its codex/openai `gpt-6.1-sol` high-effort run `01a1c0c5-0129-7012-87aa-b4196e4de9ce`
and claude/anthropic `claude-opus-5-5` high-effort run `01a1c0d1-439c-78b2-b5fe-7a1a4b07f03c`
each passed an independently checked 974-test fixture suite. Its opt-in transport-recovery campaign
also passed. That is **historical evidence for the earlier candidate**, not a fresh provider check
of this commit-message follow-up. The worker's current environment has no `E2E_OPENAI_API_KEY`
or `E2E_ANTHROPIC_API_KEY`; the prescribed independent provider checks remain required in the
behavior review environment. Its full record is `$ZEROSHOT_TOOLS/verifier-1409/summary.md`.

The saved GitHub merge delivery is unchanged. Required external CI remains pending, including
the PR/merge aggregate, applicable macOS/Windows and coverage lanes, Opcore and packaging/image
checks. No local result waives those gates. Existing #1454 bounded/redacted API diagnostics and
destination/head identity checks remain intact.
