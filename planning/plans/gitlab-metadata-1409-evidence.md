# GitLab metadata regression validation — issue #1409

The latest result is recorded under [Resolved captured-main merge](#resolved-captured-main-merge).
Earlier sections retain historical evidence.

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

## Captured-main integration follow-up

Trusted delivery integrated captured target `61b648f1f611f725cae59c8ad6866c85dcb56206` in merge
`0465897b50d1ad25393058779f048fa41b75b740`. The resulting checkout was inspected against that target,
with no unresolved conflicts or additional product changes needed. Upstream co-author attribution,
CLI credential forwarding and update recovery remain integrated. Every GitLab adapter file matches
the latest accepted independent review; `source-comparison.json` records the comparison.

The merged candidate was rebuilt with the same lean Cargo settings and `--frozen`:
`$ZEROSHOT_TOOLS/bin/zeroshot-1409-merged`, SHA-256
`b7364126de332ddbb74b075e4d59974296bacbb5de8a4ee0d7d5fdad9a395335`.
Build provenance and current logs are under `$ZEROSHOT_TOOLS/merge-1409/`.
Root/UI `npm ci`, the repository `.venv` manifest/lockfile installation above,
`.venv/bin/python -m pip check`, and `cargo fetch --locked` all completed successfully.
Manifests and lockfiles remain unchanged; ignored dependencies remain installed.
The prepared fixture's recorded revision and HEAD again matched
`998c66fa78a9ee21a1b79466f7dc7d1c53cf47cf` before the build, and its complete before/after byte
fingerprints match. Validation used separate copies.

Fresh checks against the merged checkout:

| Command                                                                                               | Result                                         |
| ----------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| `cargo build --frozen -p zeroshot --bin zeroshot`                                                     | exit 0; frozen merged candidate above          |
| `cargo test --frozen -p zeroshot --lib gitlab -- --test-threads=2`                                    | 72 passed, including existing API diagnostics  |
| `cargo test --frozen -p zeroshot --lib native_v2_delivery:: -- --test-threads=2`                      | 218 passed; 1 existing opt-in campaign ignored |
| `cargo test --frozen -p zeroshot --lib native_v2_comment_delivery:: -- --test-threads=2`              | 46 passed                                      |
| `cargo test --frozen -p zeroshot --lib native_v2_cli::templates:: -- --test-threads=2`                | 39 passed                                      |
| `cargo clippy --frozen -p zeroshot --all-targets -- -D warnings`                                      | exit 0                                         |
| `cargo run --quiet --frozen -p openengine-cluster-testkit --bin generate-cluster-protocol -- --check` | exit 0                                         |
| `npm run lint`, `npm test`, `npm run distribution:check`                                              | exit 0; 137 tooling tests passed               |
| `.venv/bin/python -m mkdocs build --strict --site-dir ARTIFACT/site`                                  | exit 0                                         |
| `cargo fmt --all -- --check`, changed Markdown Prettier checks, `git diff --check`                    | exit 0                                         |

`cargo test --frozen --workspace -- --test-threads=2` also exited 0, including complete workspace,
integration and doc tests. The main library passed 2440 tests with 13 existing opt-in tests ignored.
`cargo clippy --frozen --workspace --all-targets -- -D warnings` and
`RUSTDOCFLAGS=-Dwarnings cargo doc --frozen --workspace --no-deps` both exited 0.
`native-lane-checks.json` records these completed commands and their logs.

The focused Rust checks used one build job, two test threads and disabled debug information and
incremental compilation. Loopback tooling again used the existing private Git wrapper and original
bash/gh executables. An initial tooling invocation was stopped while waiting for the build lock
and restarted with the shared lean Cargo profile; the complete 137-test rerun passed.
The complete native checks use a real private executable temporary directory under `/var/tmp`.
An initial full-suite invocation inherited the Platform's aliased `TMPDIR`, causing private-storage
tests to reject it with `UnsafeStorage`; it was stopped and restarted with the required directory.
`tmpdir-reproduction.json` records the resolved Platform path and the same current test passing
with the corrected temporary-directory preparation. Product source was unchanged.

Fresh merged-candidate live validation confirmed GitLab **19.6.0-pre**, revision `19ecfd260d5`,
and the declared account. It used the same declared `GITLAB_TOKEN` connection and trusted delivery
launcher described above, rebuilt against the merged candidate. The exact execution was:

```sh
.venv/bin/python "$ZEROSHOT_TOOLS/merge-1409/live.py"
```

Both cases completed create, tracked-file-changing repair push/update, and unchanged retry:

| Case                 | Disposable MR                                                                           | Stored title                 | Run ID                                 |
| -------------------- | --------------------------------------------------------------------------------------- | ---------------------------- | -------------------------------------- |
| `Draft: Ready title` | [!1](https://gitlab.com/michael.eichelbeck/zs1409-merged-155762b426/-/merge_requests/1) | `Ready title`, 11 characters | `01a1c13e-6249-7be0-9f3e-70bec8280186` |
| `界🦀` × 150         | [!2](https://gitlab.com/michael.eichelbeck/zs1409-merged-155762b426/-/merge_requests/2) | first 255 Unicode characters | `01a1c13f-8079-72d1-9521-ecfb8b195c6b` |

All six deliveries succeeded. After GitLab recorded each repair SHA's diff version and push system
note, repeated API samples stayed open and nondraft. Actual repair commit messages matched the
canonical titles. Both raw slash lines, existing escapes and code examples stayed literal without
duplicate escapes, notes or close/reopen side effects; surrounding human text was preserved.
The Unicode original appeared exactly once, and retry kept the preceding SHA and description.
Each separate fixture copy also passed all 972 tests. Commands, stored API responses, commits,
diff versions, notes and rendered Markdown are in `live-evidence.json`.
`confirmed-delivery-receipts.json` separately confirms all six ready delivery completions from
the native run ledgers, two succeeded terminal results, and unchanged retry heads.

Cleanup readback marked newly created private project `88713894` for deletion on `2026-11-09`;
no existing project was changed. Private connections and the temporary askpass executable were
removed. `integrity.json` records the clean credential scan and unchanged prepared fixture.

The latest accepted independent pre-merge behavior review is
`$ZEROSHOT_TOOLS/verifier-1409-current/summary.md`: candidate SHA-256
`74af7d90abdb4b1770111cd74e6999d748e56fe10e5132b7ceb0b47c79b37ee1`, 12 live GitLab scenarios,
32 deliveries, and fresh high-effort Codex/OpenAI `gpt-6.1-sol` and Claude/Anthropic
`claude-opus-5-5` runs with 979 independently checked fixture tests each. Its disposable projects
were scheduled for deletion and private credentials removed. Those accepted results describe the
pre-merge candidate; they are separate from this follow-up's fresh merged-candidate results.
The worker still lacks both provider E2E keys. No Greptile suggestions were supplied in the
delivery feedback. Required external CI and the saved GitHub merge delivery remain unchanged.

## Current checkout validation

This validation freshly inspected and built submitted checkout
`0465897b50d1ad25393058779f048fa41b75b740`, including the integrated upstream target
`61b648f1f611f725cae59c8ad6866c85dcb56206`. The adapter implementation, acceptance-verifier guidance,
forge-delivery documentation and existing #1454 diagnostics were already present and remain intact.
The additional source change extends `gitlab/metadata_tests.rs` with combined long/draft titles
and replay of metadata read back from the MR. Reusing that canonical title/body causes no extra
write or original-title note. Earlier sections describe historical candidates, not this result.

Current candidate: `/var/lib/zeroshot/native-v2/runs/2ad0b080c25823e816e2c7ac108a5114568e55373262f653025675403cfe56b4/tools/bin/zeroshot-1409-current`.
SHA-256: `afa1134d2aa7fd87515ce73737c8ee790c673510789bf83e3fe675dfdaca5726`. CLI version: development placeholder `0.1.0`.
The validation launcher links this candidate build's library and runs this frozen executable as
its controller. Build commands, source hashes, dependency preparation and the unchanged API
error files are recorded in `$ZEROSHOT_TOOLS/current-1409/build-provenance.json`.

Preparation completed successfully: root and UI `npm ci --no-audit --no-fund`,
`.venv/bin/python -m pip install -e 'sdks/python[dev]' -r docs/requirements.lock`,
`.venv/bin/python -m pip check`, and `cargo fetch --locked`. Manifests and lockfiles are unchanged;
the ignored dependencies remain installed. Rust 1.97.0, Node 24.22.1 and Python 3.12.3 were used.
The pinned fixture's recorded revision, HEAD and supplied revision matched
`998c66fa78a9ee21a1b79466f7dc7d1c53cf47cf` before expensive work. Its before/after byte fingerprints match.
The separate seed copy passed all 972 fixture tests; every scenario used a separate repository
copy and private CLI state. The prepared fixture remains unchanged.

### Live GitLab results

The supplied `gitlab-validation-1409-20261109` connection declared `GITLAB_TOKEN` on the actual
delivery binding. A separate private connection supplied Git transport credentials. Within the
validation graph, credentials stayed with trusted delivery and out of script environments, URLs,
arguments, authored metadata and Git history.
`GET /api/v4/version` freshly returned **19.6.0-pre**, revision
`d5f9efe9028`; the user API confirmed `michael.eichelbeck` / `15584353`.
New private project: [michael.eichelbeck/zs1409-current-9df883d3c295](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295),
ID `88718234`. No pre-existing user project was modified.

```sh
CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  cargo build --frozen -p zeroshot --lib --bin zeroshot --message-format=json
.venv/bin/python "$ZEROSHOT_TOOLS/current-1409/live.py"

$ZEROSHOT_TOOLS/current-1409/cli run --title 'GitLab 1409 CASE' \
  --graph CASE/graph.json --runtime-config CASE/runtime.json --input CASE/input.json \
  --cwd CASE/repo --source-connection gitlab-validation-transport-1409 \
  --source-provider source.gitlab --source-base-url https://gitlab.com
```

The deterministic custom graph revisits the same `builtin.git-delivery.pr@2` node three times:
create/commit/push, tracked-file-changing repair push/update, then unchanged retry. All **27**
durable delivery completions are `ready`, all nine CLI runs succeeded, and each retry retained
the preceding head and description. The launcher uses the public `LocalCliBackend` seam and
Platform's existing bash egress wrapper to restore the proxy for the detached controller;
the candidate owns admission, execution, Git transport and every MR create/update operation.

| Authored title                  | Disposable MR                                                                              | Stored title                  | Characters | Draft / state  | Run ID                                 |
| ------------------------------- | ------------------------------------------------------------------------------------------ | ----------------------------- | ---------: | -------------- | -------------------------------------- |
| `fix: publish literal metadata` | [!1](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/1) | fix: publish literal metadata |         29 | false / opened | `01a1c198-9fb9-7301-bf02-f14b1685227d` |
| `a` × 300                       | [!2](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/2) | first 255 Unicode characters  |        255 | false / opened | `01a1c199-b84d-7623-9876-87c6de7a6349` |
| `界🦀` × 150                    | [!3](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/3) | first 255 Unicode characters  |        255 | false / opened | `01a1c19a-c209-7691-a4a2-0cc4cda07e96` |
| `Draft: Ready title`            | [!4](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/4) | Ready title                   |         11 | false / opened | `01a1c19b-9b86-74b1-89ff-21dfec00d45f` |
| `[Draft] Ready title`           | [!5](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/5) | Ready title                   |         11 | false / opened | `01a1c19c-c10d-7c61-b159-47be327f1415` |
| `(Draft) Ready title`           | [!6](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/6) | Ready title                   |         11 | false / opened | `01a1c19d-8644-7353-8dc9-470ecf8a2454` |
| `Draft:`                        | [!7](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/7) | Zeroshot change               |         15 | false / opened | `01a1c19e-df44-7281-a20c-5bfc569940f4` |
| `[Draft]`                       | [!8](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/8) | Zeroshot change               |         15 | false / opened | `01a1c19f-fb68-74c0-996b-b738c6dc38c7` |
| `(Draft)`                       | [!9](https://gitlab.com/michael.eichelbeck/zs1409-current-9df883d3c295/-/merge_requests/9) | Zeroshot change               |         15 | false / opened | `01a1c1a1-f9dc-7bf3-ad96-59a4cd2f8bff` |

Every generated description supplied raw standalone `/health returns 200` and `/close`, an
already escaped slash, inline code, backtick and tilde fences, and an indented example. Actual
stored lines had one backslash after every delivery; existing escapes never doubled. GitLab's
Markdown API rendered the prose as literal `/health returns 200` and `/close` without visible
backslashes. Code examples retain the documented conservative escape. The original 300-character
ASCII and multibyte titles each occurred exactly once inside the generated section.
Human text added outside the markers was preserved exactly. No close/reopen system event occurred.

For every repair, the API confirmed the actual commit message equaled the canonical title and
its parent was the creation SHA. The matching MR diff version and added-commit system note were
observed before four additional open/nondraft samples. The final branch API head matched the final
retry receipt; earlier receipts matched their corresponding sampled creation and repair SHAs. Complete stored metadata, Markdown, commands, commits, diff versions,
system notes, status and all 27 native completions are in
`$ZEROSHOT_TOOLS/current-1409/live-evidence.json`; graphs and native logs remain alongside it.

Validation preparation exposed GitLab's existing asynchronous MR/head lag on an unpaced repair
push. The validation-only Git wrapper delegates every real command unchanged and waits three
seconds after a successful push, allowing GitLab's push worker to settle before the candidate
reads it. All destination/head checks still run; no product polling, deadlines or identity checks
were changed. Earlier harness attempts also corrected an expectation about GitLab trimming
trailing description whitespace and removed an executable continuation after the final delivery,
which correctly invalidated its receipt. Their logs are retained under `preparation*`; they are
not counted as passing runs. All three preparation projects were scheduled for deletion.

Current project cleanup readback reports
`michael.eichelbeck/zs1409-current-9df883d3c295-deletion_scheduled-88718234`, marked for deletion on
`2026-11-09`. Connection stores and the temporary askpass
executable were removed. The credential scan passed, and the prepared fixture's fingerprint
remains `1ff2bee700da12298a1d73de73c9ebd8e7fc75dc3f6d086786d7c945968daab5`.

### Fresh local checks and remaining gates

All Rust checks used one build job, disabled debug information and incremental compilation, and
two test threads. Native fixture tests used a real private executable `TMPDIR` under `/var/tmp`.
Their loopback-only PATH used the original bash/gh and a Git wrapper that excludes the Platform
system proxy only when no test-owned `GIT_CONFIG_SYSTEM` was provided. Live GitLab retained the
Platform proxy.

| Command                                                                                               | Result                                                                                                     |
| ----------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `cargo test --frozen -p zeroshot --lib gitlab -- --test-threads=2`                                    | 72 passed, including #1454 diagnostics and expanded metadata cases                                         |
| `cargo test --frozen -p zeroshot --lib native_v2_delivery:: -- --test-threads=2`                      | 218 passed; 1 existing opt-in campaign ignored                                                             |
| `cargo test --frozen -p zeroshot --lib native_v2_comment_delivery:: -- --test-threads=2`              | 46 passed, including normalized publication hashes and lost responses                                      |
| `cargo test --frozen -p zeroshot --lib native_v2_cli::templates:: -- --test-threads=2`                | 39 passed                                                                                                  |
| `cargo test --frozen --workspace -- --test-threads=2`                                                 | exit 0; workspace, integrations and doc tests; main library 2440 passed / 13 existing opt-in tests ignored |
| `cargo clippy --frozen --workspace --all-targets -- -D warnings`                                      | exit 0                                                                                                     |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --frozen --workspace --no-deps`                                    | exit 0                                                                                                     |
| `npm run lint`, `npm test`, `npm run distribution:check`                                              | exit 0; 137 tooling tests passed                                                                           |
| `cargo run --quiet --frozen -p openengine-cluster-testkit --bin generate-cluster-protocol -- --check` | exit 0                                                                                                     |
| `.venv/bin/python -m mkdocs build --strict`                                                           | exit 0                                                                                                     |

Commands, exit statuses and logs are recorded in `$ZEROSHOT_TOOLS/current-1409/checks.json`.
Final Rust/Markdown formatting and whitespace checks are recorded in `final-checks.json` there.
The worker environment does not supply `E2E_OPENAI_API_KEY` or `E2E_ANTHROPIC_API_KEY`; required
independent real-provider journeys remain for the behavior review environment. Historical provider
results above are not current passes. Required external CI, including `CI / required`, remains
pending; the saved GitHub merge delivery and its other gates are unchanged. No Greptile feedback
was supplied, so there are no suggestions to apply or dismiss.

## Latest captured-main integration

Trusted delivery integrated captured target `dc44b68d87d0b8b210a42da8ce7265208db71788` in merge `c198e0f7e4e99a57a9b990d62621eeb82f9822d3`. Inspection confirmed that the target is an ancestor, there are no unresolved conflicts, and all upstream catalog, provider-access, UI, Docker and CI changes remain integrated. The issue-specific production files, regression tests, acceptance guidance and forge documentation are byte-identical to reviewed candidate `afa1134d2aa7fd87515ce73737c8ee790c673510789bf83e3fe675dfdaca5726`. The API implementation and diagnostic tests from #1454 also remain unchanged against the captured target. No corrective product edits were needed; this follow-up adds current evidence.

Candidate: `$ZEROSHOT_TOOLS/bin/zeroshot-1409-integrated`, built from the merged checkout above with `cargo build --frozen -p zeroshot --lib --bin zeroshot --message-format=json`. SHA-256: `9e95ec32a7ca77b943697b6e2c987adb2ba99460e0466e9de0e4f102ffb2d87b`. The candidate reports `zeroshot 0.1.0`, the checked-in development placeholder. Build provenance, commands, exit statuses, logs and live API evidence are under `$ZEROSHOT_TOOLS/integrated-1409-dc44b68d/`.

Setup completed successfully: root and UI `npm ci --no-audit --no-fund`, `.venv/bin/python -m pip install -e 'sdks/python[dev]' -r docs/requirements.lock`, `.venv/bin/python -m pip check`, `cargo fetch --locked`, and `npm --prefix ui run build`. Manifests and lockfiles are unchanged; installed dependencies and refreshed ignored UI assets remain available. The prepared fixture's sibling `zeroshot-matrix.REVISION`, `ZEROSHOT_MATRIX_REVISION` and Git HEAD match `998c66fa78a9ee21a1b79466f7dc7d1c53cf47cf`. Its complete before/after fingerprint remains `1ff2bee700da12298a1d73de73c9ebd8e7fc75dc3f6d086786d7c945968daab5`; all live work used separate copies, and the fresh seed passed 972 tests.

Fresh checks on the merged checkout:

| Command                                                                                               | Result                                                                                            |
| ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `cargo test --frozen -p zeroshot --lib gitlab -- --test-threads=2`                                    | 72 passed; existing #1454 diagnostics included                                                    |
| `cargo test --frozen -p zeroshot --lib native_v2_delivery:: -- --test-threads=2`                      | 218 passed; 1 existing opt-in ignored                                                             |
| `cargo test --frozen -p zeroshot --lib native_v2_comment_delivery:: -- --test-threads=2`              | 46 passed                                                                                         |
| `cargo test --frozen -p zeroshot --lib native_v2_cli::templates:: -- --test-threads=2`                | 39 passed                                                                                         |
| `cargo test --frozen --workspace -- --test-threads=2`                                                 | exit 0; main library 2440 passed / 13 existing opt-ins ignored; integrations and doc tests passed |
| `cargo test --frozen -p zeroshot --features ui --lib workspace:: -- --test-threads=2`                 | 80 passed, including integrated catalog and setup tests                                           |
| `cargo test --frozen -p zeroshot --features ui --lib profile_ui:: -- --test-threads=2`                | 73 passed                                                                                         |
| `cargo clippy --frozen --workspace --all-targets --all-features -- -D warnings`                       | exit 0                                                                                            |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --frozen --workspace --all-features --no-deps`                     | exit 0                                                                                            |
| `npm --prefix ui test`, `npm --prefix ui run build`                                                   | 450 tests passed; asset/type build exit 0                                                         |
| `npm run lint`, `npm test`, `npm run distribution:check`                                              | exit 0 each; 137 tooling tests passed                                                             |
| `cargo run --quiet --frozen -p openengine-cluster-testkit --bin generate-cluster-protocol -- --check` | exit 0                                                                                            |
| `.venv/bin/python -m mkdocs build --strict --site-dir ARTIFACTS/site`                                 | exit 0                                                                                            |
| `opcore check --repo . --committed --comparison all --providers fast`                                 | exit 0; 0 diagnostics; checksum-pinned Opcore 0.3.2                                               |
| `cargo fmt --all -- --check`                                                                          | exit 0                                                                                            |

Rust checks used one build job, two test threads, debug information disabled and incremental compilation disabled. The effective prepared Cargo cache is `/var/lib/zeroshot/native-v2/runs/2ad0b080c25823e816e2c7ac108a5114568e55373262f653025675403cfe56b4/runtime/writer-node-instance-1/.cargo`. Local subprocess fixtures used `$ZEROSHOT_TOOLS/current-1409/loopback-bin` on `PATH`, the real private executable `TMPDIR=/var/tmp/zs1409-integrated-tests`, and `ZEROSHOT_RESTIC=$ZEROSHOT_TOOLS/zeroshot-restic/x86_64-unknown-linux-musl/restic`. Live GitLab retained Platform egress. These are fresh completed commands, rather than historical results.

Fresh real GitLab execution:

```sh
.venv/bin/python "$ZEROSHOT_TOOLS/integrated-1409-dc44b68d/live.py"
$ZEROSHOT_TOOLS/integrated-1409-dc44b68d/cli run --title "GitLab 1409 CASE" --graph CASE/graph.json --runtime-config CASE/runtime.json --input CASE/input.json --cwd CASE/repo --source-connection gitlab-validation-transport-1409 --source-provider source.gitlab --source-base-url https://gitlab.com
```

Authenticated `/version` confirmed GitLab **19.6.0-pre**, revision `b53243fc6a6`; `/user` confirmed `michael.eichelbeck`, ID `15584353`. The new private project was [michael.eichelbeck/zs1409-integrated-cc574108d9b7](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7), ID `88720353`. The API credential came through declared connection `gitlab-validation-1409-20261109`; Git used its separate declared transport connection. The launcher was rebuilt against this candidate's library and uses the frozen candidate controller through the existing Platform Bash egress wrapper. Actual Git commits/pushes and GitLab MR create/update/rediscovery used the trusted delivery path. The validation-only Git wrapper retains its three-second wait after successful pushes for existing MR/head propagation lag; product identity checks and polling behavior were unchanged.

| Case               | MR                                                                                              | Stored title characters | Run ID                                 |
| ------------------ | ----------------------------------------------------------------------------------------------- | ----------------------: | -------------------------------------- |
| literal            | [!1](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/1)   |                      29 | `01a1c1c9-02b7-7602-b5ac-db0a3e9ae239` |
| long-ascii         | [!2](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/2)   |                     255 | `01a1c1c9-ba00-7511-9832-f79cf31ada72` |
| long-unicode       | [!3](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/3)   |                     255 | `01a1c1ca-cccf-7822-82a8-216daf91d709` |
| draft-colon        | [!4](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/4)   |                      11 | `01a1c1cb-8eb0-7ae2-b990-4105fcfd81ea` |
| draft-bracket      | [!5](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/5)   |                      11 | `01a1c1cc-535f-7680-a9c4-38e190665d5c` |
| draft-paren        | [!6](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/6)   |                      11 | `01a1c1cd-0adb-7252-8d1f-e740fe64587a` |
| only-colon         | [!7](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/7)   |                      15 | `01a1c1ce-1cf1-7420-aaae-4c8e6c8749a1` |
| only-bracket       | [!8](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/8)   |                      15 | `01a1c1cf-2eb4-7612-8e4a-f0901bb4c2d9` |
| only-paren         | [!9](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/9)   |                      15 | `01a1c1d0-3b53-7a72-8eca-d722207bec32` |
| long-draft-ascii   | [!10](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/10) |                     255 | `01a1c1d0-fd77-7181-a8b0-bb39feac3ee9` |
| long-draft-unicode | [!11](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/11) |                     255 | `01a1c1d1-b2a0-7983-9c5c-f444078b261c` |
| repeat-draft       | [!12](https://gitlab.com/michael.eichelbeck/zs1409-integrated-cc574108d9b7/-/merge_requests/12) |                      11 | `01a1c1d3-1109-7c83-9451-fee18bb4a488` |

All 12 cases completed creation, a tracked-file-changing repair push/update, and an unchanged retry: **36 ready delivery completions and 12 durably succeeded terminal results**. Stored prose contains single backslash escapes for `/health returns 200` and `/close`; pre-existing escapes remain single. GitLab rendered the prose as literal text without visible escapes. Code examples retain the documented conservative escapes. ASCII and multibyte originals, including combined draft/long inputs, occur exactly once in the generated section. Prefix-only titles become `Zeroshot change`; all three draft prefixes and repeated prefixes produce ready titles. Every stored title is nonempty and at most 255 Unicode characters.

Human text outside the generated markers remained exact. Each repair commit message equals the normalized title, and its parent is the creation SHA. Matching MR diff versions and added-commit system notes were observed before four further open/nondraft API samples; no close/reopen side effects occurred. Final retries retained the same MR, head and description; final native receipts matched the API branch heads. A separate API read confirmed exactly 12 MRs, all open and nondraft, and zero disposable-project pipelines. `post-validation.json` records these additional checks.

Cleanup DELETE was accepted for newly created project `88720353`. Readback confirmed `michael.eichelbeck/zs1409-integrated-cc574108d9b7-deletion_scheduled-88720353`, marked for deletion on `2026-11-09`. Private connections and temporary askpass were removed; credential scans passed, and the pinned fixture was unchanged. No pre-existing project was modified.

Latest accepted independent feedback reports no code or behavior defects. `$ZEROSHOT_TOOLS/verify-current-1409/summary.md` records the preceding reviewed candidate (`afa1134d2aa7...`): 12 metadata scenarios plus two required high-effort provider journeys, with 42 ready delivery completions. Codex/OpenAI `gpt-6.1-sol` run `01a1c1b1-9d8c-7593-87c2-cd3a583a8756` passed 982 fixture tests; Claude/Anthropic `claude-opus-5-5` run `01a1c1b3-7e48-7093-8782-b150871ddc5e` passed 980. Their private projects were scheduled for deletion and credential scans passed. Those provider results retain their preceding-candidate provenance; this worker still lacks both provider E2E keys. The optional transport campaign stopped at its existing fixture precondition and is not counted as passing.

The integrated candidate above has fresh local and live GitLab results. Independent review and required external CI remain separate gates; no external CI pass is asserted. No Greptile suggestions were supplied in the latest delivery feedback. The saved GitHub merge delivery remains unchanged.

## Resolved captured-main merge

Trusted delivery next attempted to merge captured target `03e44f188cb75e561e6be2f29be4f2c449686397`. The sole conflict in `docs/guides/forge-delivery.md` is resolved in merge `154a40cb6dc6cc2f9d5f8d8c25cde0d5b07b7cf6`: the guide retains both this issue's metadata behavior and upstream's explicit optional-CI policy. All integrated upstream plugin packages, manifests, CI-policy implementation and six CI regressions remain intact. The target is an ancestor; no unresolved index entries remain. Existing bounded/redacted API diagnostics remain unchanged against that target. No further product correction was required.

Candidate: `$ZEROSHOT_TOOLS/bin/zeroshot-1409-resolved`, SHA-256 `8de933926416aac324ed6e61bae61b12aa19a870191cf9223055e7b4d2dda2d6`, built from the resolved merge with `cargo build --frozen -p zeroshot --lib --bin zeroshot --message-format=json` (exit 0). It reports the development placeholder `zeroshot 0.1.0`. The matching library-backed CLI launcher and frozen controller are under `$ZEROSHOT_TOOLS/resolved-1409-03e44f18/`; build provenance records source hashes and exact commands. This evidence update is the only post-build source change.

Repository setup completed with exit 0 for root and UI `npm ci --no-audit --no-fund`, `.venv/bin/python -m pip install -e 'sdks/python[dev]' -r docs/requirements.lock`, `.venv/bin/python -m pip check`, `cargo fetch --locked`, and `npm --prefix ui run build`. No worker changes to manifests or lockfiles were made. Ignored installed dependencies and generated UI assets remain available. Rust checks use one build job, two test threads, no debug information or incremental compilation, the prepared Cargo cache, real executable `TMPDIR=/var/tmp/zs1409-resolved-tests`, the existing loopback test wrappers and prepared Restic executable described above. Live validation retains Platform egress and uses separate native CLI state and repository copies.

Before builds, the fixture's recorded revision, supplied revision and HEAD again matched `998c66fa78a9ee21a1b79466f7dc7d1c53cf47cf`, with fingerprint `1ff2bee700da12298a1d73de73c9ebd8e7fc75dc3f6d086786d7c945968daab5`. `npm run plugins:check -- --base 03e44f188cb75e561e6be2f29be4f2c449686397`, checksum-pinned Opcore Fast, `cargo fmt --all -- --check` and `npm run lint` all exited 0; Opcore reported zero diagnostics.

The first two campaigns on this merge are retained as **failed attempts**, rather than successful validation. In `$ZEROSHOT_TOOLS/resolved-1409-03e44f18/live-evidence.json`, the first five complete scenarios passed, then the `(Draft)` repair push hit the existing bound-review/head consistency refusal. The disposable project was `michael.eichelbeck/zs1409-resolved-9aeac91637d9` (ID `88723378`). A second attempt under `after-head-lag/` failed at the first literal scenario's repair push in `michael.eichelbeck/zs1409-resolved-retry-761da2ea319f` (ID `88723682`). Settled API readbacks showed the expected repair commit at both the MR and branch head, with the MR open and nondraft; refusal occurred before updating generated metadata. Both projects were scheduled for deletion on `2026-11-09`, and credential cleanup/scans passed.

Inspection found a validation preparation error: the launcher's `VALIDATION_GIT` selects checkout preparation, while native delivery independently resolves Git from the captured `PATH`. Configuring three- or ten-second wrappers through that argument alone did not prove any delivery push pacing. The corrected campaign prepends its `transport-bin/git` wrapper to captured `PATH`; credential-free receipts now prove that **actual delivery pushes** execute the wrapper and wait ten seconds after Git returns. This is test-only pacing for the documented GitLab asynchronous head propagation; destination/head guards, product retries and deadlines remain unchanged. Earlier successful campaigns remain successful observed executions, but their configured wrapper alone is not evidence of delivery pacing.

Fresh live commands use the built candidate through its library-backed launcher:

```sh
.venv/bin/python "$ZEROSHOT_TOOLS/resolved-1409-03e44f18/with-delivery-git/live.py"
$ZEROSHOT_TOOLS/resolved-1409-03e44f18/with-delivery-git/cli run --title "GitLab 1409 CASE" --graph CASE/graph.json --runtime-config CASE/runtime.json --input CASE/input.json --cwd CASE/repo --source-connection gitlab-validation-transport-1409 --source-provider source.gitlab --source-base-url https://gitlab.com
```

The deterministic validation graph invokes actual `builtin.git-delivery.pr@2` three times per case: creation, a tracked-file-changing repair push with updated metadata, then an unchanged retry. The API connection explicitly declares `gitlab-validation-1409-20261109` with `GITLAB_TOKEN`; Git uses a separate declared transport connection. Credentials enter only private connection storage through stdin, are never in arguments or URLs, and are removed after validation. Authentication confirmed GitLab `19.6.0-pre`, revision `b53243fc6a6`, and `michael.eichelbeck` (user `15584353`). All resources are new private disposable projects in that namespace.

The latest accepted independent review predates this target merge: `$ZEROSHOT_TOOLS/verify-integrated-1409/summary.md` identifies reviewed checkout `c198e0f7e4e99a57a9b990d62621eeb82f9822d3` and rebuilt candidate SHA-256 `d22faf201ee6e30d591959697ff43f3bdd12d721221b2df643e799fb83e16f15`. It records 12 GitLab scenarios with 36 ready completions, complete local checks, and both required high-effort provider journeys: Codex/OpenAI `gpt-6.1-sol`, run `01a1c1ea-1932-7383-ab6e-d2aff19b5b07`, 981 fixture tests; Claude/Anthropic `claude-opus-5-5`, run `01a1c1ed-3789-7603-8360-027fa04e557c`, 980 tests. A validation-only HTML assertion was corrected and independently rechecked; the original harness failure remains recorded there. Neither reviewer found an actionable product defect. These results retain their preceding-candidate provenance. This worker has neither provider E2E key and does not claim fresh provider journeys on the resolved merge. Current independent review and external CI remain required gates. No Greptile suggestions were supplied, and the saved GitHub merge delivery is unchanged.

Fresh completed checks on the resolved checkout:

| Command                                                                                               | Result                                                                                           |
| ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `cargo test --frozen -p zeroshot --lib gitlab -- --test-threads=2`                                    | 78 passed, including the six integrated optional-CI regressions and #1454 diagnostics            |
| `cargo test --frozen -p zeroshot --lib native_v2_delivery:: -- --test-threads=2`                      | 218 passed; 1 existing opt-in ignored                                                            |
| `cargo test --frozen -p zeroshot --lib native_v2_comment_delivery:: -- --test-threads=2`              | 46 passed                                                                                        |
| `cargo test --frozen -p zeroshot --lib native_v2_cli::templates:: -- --test-threads=2`                | 39 passed                                                                                        |
| `cargo test --frozen --workspace -- --test-threads=2`                                                 | exit 0; main library 2446 passed / 13 existing opt-ins ignored; integration and doc tests passed |
| `cargo clippy --frozen --workspace --all-targets -- -D warnings`                                      | exit 0; warnings denied                                                                          |
| `cargo doc --frozen --workspace --no-deps`                                                            | exit 0; RUSTDOCFLAGS=-Dwarnings                                                                  |
| `npm test`                                                                                            | 150 passed                                                                                       |
| `npm run distribution:check`                                                                          | exit 0                                                                                           |
| `cargo run --quiet --frozen -p openengine-cluster-testkit --bin generate-cluster-protocol -- --check` | exit 0; generated files unchanged                                                                |
| `.venv/bin/python -m mkdocs build --strict --site-dir ARTIFACTS/site`                                 | exit 0; strict site build                                                                        |

The full workspace command finished in 1203 seconds. Its 120 successful test summaries total 3510 passed and 23 existing ignored tests. The disposable seed independently passed all 972 pinned-fixture tests. Full commands, exit statuses and logs are recorded in `setup.json`, `preliminary-checks.json`, `build-provenance.json` and `checks.json` under the artifact directory above.

The corrected live campaign passed all **12 scenarios, 36 native ready completions and 12 durably succeeded terminal results** in [michael.eichelbeck/zs1409-final-0ece9a5f4991](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991), private project `88723841`. Each scenario performed three real Git pushes, including the unchanged-head retry; the transport receipts prove all 36 actual delivery pushes used the ten-second wrapper.

| Case               | MR                                                                                         | Stored title characters | Run ID                                 |
| ------------------ | ------------------------------------------------------------------------------------------ | ----------------------: | -------------------------------------- |
| literal            | [!1](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/1)   |                      29 | `01a1c21c-c653-7611-bbdd-c5d3b6d2c999` |
| long-ascii         | [!2](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/2)   |                     255 | `01a1c21e-7635-7b73-9aae-742c1054c342` |
| long-unicode       | [!3](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/3)   |                     255 | `01a1c21f-caf6-7953-b63b-3822431e3d13` |
| draft-colon        | [!4](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/4)   |                      11 | `01a1c221-0f95-7f71-9046-f15b8acda270` |
| draft-bracket      | [!5](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/5)   |                      11 | `01a1c223-164c-7041-8ece-c40ec23087a2` |
| draft-paren        | [!6](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/6)   |                      11 | `01a1c224-b275-7841-b481-fecd28a76bb9` |
| only-colon         | [!7](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/7)   |                      15 | `01a1c226-44a8-7371-97bb-58ac1ddd1860` |
| only-bracket       | [!8](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/8)   |                      15 | `01a1c228-2506-7a73-bde6-832adcb18f8d` |
| only-paren         | [!9](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/9)   |                      15 | `01a1c229-6726-7b40-9584-f8ff77fce042` |
| long-draft-ascii   | [!10](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/10) |                     255 | `01a1c22a-f3d7-7e53-889b-51ca0beb3ef4` |
| long-draft-unicode | [!11](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/11) |                     255 | `01a1c22c-3205-7ef2-bd5b-a56c50463db9` |
| repeat-draft       | [!12](https://gitlab.com/michael.eichelbeck/zs1409-final-0ece9a5f4991/-/merge_requests/12) |                      11 | `01a1c22d-63e5-7440-8a09-53181cec4c41` |

Live API checks confirmed literal `/health returns 200` and `/close` prose; the existing escape stays single, and code examples retain the documented conservative escape. Parsed rendered paragraphs contain readable slash text without visible backslashes. Both 300-character titles and the combined long/draft cases store 255 Unicode characters and retain the exact authored original once. Every supported prefix, prefix-only input and repeated-prefix input produces a nonempty nondraft title. Repair commit messages equal those canonical titles; matching diff versions and added-commit system notes settled before four further open/nondraft samples. No close/reopen system note was observed. Human prefix/suffix text remains exact, and repeated metadata delivery neither adds escapes/notes nor changes the final head or description.

An additional API readback confirmed all 12 MRs open/nondraft with the recorded final metadata, exactly 12 MRs in the project, and no pipelines. Each native completion's MR/head identity matches its observed API revision. `with-delivery-git/post-validation.json` records the successful independent readback (command: `.venv/bin/python "$ZEROSHOT_TOOLS/resolved-1409-03e44f18/with-delivery-git/post-validate.py"`, exit 0). The initial post-check incorrectly expected only creation and repair pushes; receipts showed the third unchanged-head retry push too. That validation-only assertion was corrected to verify all three real pushes and their matching heads/waits. Its initial failure and validator are retained in `post-validation-initial-failure.txt` and `post-validate-initial.py`; no product source changed.

Cleanup DELETE for project `88723841` was accepted. Readback confirms `michael.eichelbeck/zs1409-final-0ece9a5f4991-deletion_scheduled-88723841`, marked for deletion on `2026-11-09`. Along with the two failed-attempt projects above, every newly created test project is scheduled for deletion. Private connections and askpass were removed; credential scans passed. The live helper finished, every child CLI returned 0, and the pinned fixture's complete before/after fingerprint remains unchanged. No existing user project was modified. Current candidate source and binary integrity checks pass; only this evidence document changed after the build. Independent behavior/code review and required external CI remain separate gates, with no external CI pass asserted.
