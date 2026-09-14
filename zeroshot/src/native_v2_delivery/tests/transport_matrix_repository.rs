//! Opt-in campaign against a supplied, already cloned matrix repository; never contacts GitHub.
use super::*;

const COUNT_ACTIVE: &str = r#"  countActive(tenantId) {
    requireIdentifier(tenantId, 'invalid_tenant', 'tenantId');
    const now = this.#readNow();
    let count = 0;
    for (const receipt of this.#receiptsByTenant.get(tenantId)?.values() ?? []) {
      if (receipt.expiresAt > now) count += 1;
    }
    return count;
  }

"#;

const COUNT_TESTS: &str = r#"import assert from 'node:assert/strict';
import test from 'node:test';
import { GitHubDeliveryReceiptRegistry } from '../src/github-delivery-receipts.js';

function fixture() {
  let calls = 0;
  const registry = new GitHubDeliveryReceiptRegistry({ clock: () => { calls += 1; return 100; } });
  const record = (tenantId, deliveryId, expiresAt) => registry.record({
    tenantId, deliveryId, expiresAt, receivedAt: 0, issueNumber: 1,
    repository: 'acme/project', runId: 'recovery-campaign',
  });
  return { registry, record, calls: () => calls };
}

test('countActive isolates tenants and excludes the exact expiry boundary', () => {
  const { registry, record } = fixture();
  record('a', 'expired', 100);
  record('a', 'active', 101);
  record('b', 'other', 101);
  assert.equal(registry.countActive('a'), 1);
  assert.equal(registry.countActive('b'), 1);
  assert.equal(registry.countActive('absent'), 0);
});

test('countActive samples the clock once and preserves receipt identity', () => {
  const { registry, record, calls } = fixture();
  const original = record('a', 'active', 101);
  record('a', 'expired', 99);
  assert.equal(registry.countActive('a'), 1);
  assert.equal(calls(), 1);
  assert.strictEqual(record('a', 'active', 101), original);
  assert.deepEqual(registry.pruneExpired('a'), { removed: 1, remaining: 1 });
});

test('countActive validates tenant before reading the clock', () => {
  const { registry, calls } = fixture();
  assert.throws(() => registry.countActive(''), { code: 'invalid_tenant' });
  assert.equal(calls(), 0);
});
"#;

fn matrix_repository() -> TempRepo {
    let seed = std::env::var_os("ZEROSHOT_DELIVERY_MATRIX_REPOSITORY")
        .expect("set ZEROSHOT_DELIVERY_MATRIX_REPOSITORY to an existing local matrix clone");
    let mut repo = TempRepo::delivery();
    git(
        &repo.workspace,
        &[
            "fetch",
            "--no-tags",
            Path::new(&seed).to_str().assert_value(),
            "main",
        ],
    );
    git(&repo.workspace, &["checkout", "-B", "main", "FETCH_HEAD"]);
    repo.base = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    // This remote belongs only to this ephemeral fixture. The supplied seed remains read-only.
    git(&repo.workspace, &["push", "--force", "origin", "main"]);
    assert!(
        repo.workspace
            .join("src/github-delivery-receipts.js")
            .is_file()
    );
    eprintln!("matrix seed revision={}", repo.base);
    repo
}

fn write_matrix_candidate(repo: &TempRepo) {
    let module = repo.workspace.join("src/github-delivery-receipts.js");
    let text = fs::read_to_string(&module).assert_value();
    assert_eq!(text.matches("  pruneExpired(tenantId) {").count(), 1);
    assert!(
        !text.contains("  countActive(tenantId) {"),
        "matrix seed already contains campaign feature"
    );
    let candidate = text.replace(
        "  pruneExpired(tenantId) {",
        &format!("{COUNT_ACTIVE}  pruneExpired(tenantId) {{"),
    );
    fs::write(module, candidate).assert_value();
    fs::write(
        repo.workspace
            .join("test/git-recovery-count-active.test.js"),
        COUNT_TESTS,
    )
    .assert_value();
}

fn npm_test(workspace: &Path, stage: &str) {
    let output = Command::new("npm")
        .args(["test", "--", "--test-reporter=tap"])
        .current_dir(workspace)
        .output()
        .assert_value();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stage} npm test:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let total = stdout
        .lines()
        .find_map(|line| line.strip_prefix("# tests "))
        .and_then(|value| value.parse::<usize>().ok())
        .expect("node test count");
    assert!(
        total >= 865,
        "expected the full matrix baseline and three campaign regressions, got {total}"
    );
    assert!(stdout.contains("# fail 0"));
    eprintln!("matrix {stage}: {total} tests passed");
}

fn verify_published_candidate(repo: &TempRepo) {
    let checkout = repo.root.child("published-candidate");
    git(
        repo.root.path(),
        &[
            "clone",
            "--branch",
            &delivery_branch(RUN),
            repo.remote.to_str().assert_value(),
            checkout.to_str().assert_value(),
        ],
    );
    assert_eq!(
        git_output(&checkout, &["rev-parse", "HEAD"]),
        git_output(&repo.workspace, &["rev-parse", "HEAD"])
    );
    npm_test(&checkout, "independent published checkout");
}

#[tokio::test]
#[ignore = "requires ZEROSHOT_DELIVERY_MATRIX_REPOSITORY local clone and Node >=22"]
async fn actual_matrix_repository_recovers_remote_update_and_lost_receipt_over_authenticated_http()
{
    for update in [Update::Known, Update::LostResponse] {
        let repo = matrix_repository();
        write_matrix_candidate(&repo);
        npm_test(&repo.workspace, "reviewed candidate");
        let server = HttpGit::start(&repo.remote);
        let authority = Arc::new(TransportAuthority::new(&repo, &server, update));
        let adapter = adapter(&repo, authority.clone());

        let first = execute(&repo, adapter.clone(), false).await;

        if update == Update::LostResponse {
            assert_delivery_signal(&first.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
            npm_test(&repo.workspace, "reconciled candidate before repair");
            commit_file(
                &repo.workspace,
                "repair-evidence.txt",
                "repair verified reconciled candidate\n",
            );
            fs::write(
                repo.workspace.join("repair-uncommitted.txt"),
                "preserve this repair too\n",
            )
            .assert_value();
            let next = execute(&repo, adapter, false).await;
            assert_delivery_signal(&next.outcome, DELIVERY_MERGED_LABEL);
            assert_eq!(
                fs::read_to_string(repo.workspace.join("repair-uncommitted.txt")).assert_value(),
                "preserve this repair too\n"
            );
        } else {
            assert_delivery_signal(&first.outcome, DELIVERY_MERGED_LABEL);
        }
        assert_eq!(authority.updates.load(Ordering::SeqCst), 1);
        assert_eq!(server.faults.fetch_failures.load(Ordering::SeqCst), 1);
        verify_published_candidate(&repo);
    }
}
