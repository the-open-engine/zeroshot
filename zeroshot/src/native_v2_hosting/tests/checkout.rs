use std::sync::Arc;

use super::*;
use crate::native_v2_cloud::{AllocatedCapsule, CapsuleAllocationUnavailable};
use crate::native_v2_delivery::git_auth::encode_basic_credential;
use crate::native_v2_target_authority::OperatorDiagnosticStore;

const CHECKOUT_TOKEN: &str = "checkout-secret";

struct CheckoutFixture {
    _repository: RepositoryFixture,
    root: TestDirectory,
    allocator: ProductionCapsuleAllocator,
    admitted: native_v2_contract::AdmittedRun,
    diagnostics: Arc<OperatorDiagnosticStore>,
    run_id: RunId,
}

impl CheckoutFixture {
    async fn new(body: &str) -> Self {
        let repository = RepositoryFixture::new();
        let root = TestDirectory::new("checkout-recovery");
        prepare_storage_root(&root.path().to_owned()).assert_value();
        let script = format!(
            r#"#!/bin/sh
set -eu
previous=
workspace=/
for argument do
  if [ "$previous" = -C ]; then workspace=$argument; fi
  previous=$argument
done
case " $* " in
  *" fetch "*) /usr/bin/printf x >> "$0.attempts" ;;
esac
{body}
exec /usr/bin/git "$@"
"#
        );
        let program = root.write_executable("git-script", &script);
        let attempts = root.write("git-script.attempts", "");
        fs::set_permissions(attempts, fs::Permissions::from_mode(0o666)).assert_value();
        let external = root.child("git-script.external");
        fs::create_dir(&external).assert_value();
        fs::write(external.join("keep"), "outside checkout").assert_value();
        let mut config = capsule_config(root.path().to_owned());
        config.git_program = program;
        let diagnostics = config.operator_diagnostics.clone();
        let allocator = ProductionCapsuleAllocator::new(config)
            .assert_value()
            .with_test_filesystem_and_source(repository.remote.clone(), portable_filesystem);
        let admitted = admit(submission(
            runtime(BTreeSet::new()),
            &repository.main_revision,
            "checkout-recovery",
        ))
        .await;
        Self {
            _repository: repository,
            root,
            allocator,
            admitted,
            diagnostics,
            run_id: RunId::new("checkout-recovery"),
        }
    }

    async fn allocate(&self) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        self.allocator
            .allocate(&self.run_id, &self.admitted, Some(CHECKOUT_TOKEN))
            .await
    }
}

#[tokio::test]
async fn transient_fetch_failure_recovers_in_the_same_allocation_at_the_exact_revision() {
    let fixture = CheckoutFixture::new(
        r#"case " $* " in
  *" fetch "*)
    if [ "$(/usr/bin/cat "$0.attempts")" = x ]; then
      /usr/bin/git "$@"
      /usr/bin/mkdir "$workspace/partial"
      /usr/bin/ln -s "$0.external" "$workspace/partial/external"
      /usr/bin/printf stale > "$workspace/.git/index.lock"
      /usr/bin/printf 'temporary remote disconnect\n' >&2
      exit 42
    fi ;;
esac"#,
    )
    .await;
    let capsule = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    let head = std::process::Command::new("/usr/bin/git")
        .arg("-C")
        .arg(&workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .assert_value();
    assert!(head.status.success());
    assert_eq!(
        String::from_utf8(head.stdout).assert_value().trim(),
        fixture.admitted.source.revision.as_str()
    );
    assert_eq!(fixture.root.read("git-script.attempts"), "xx");
    assert!(!workspace.join("partial").exists());
    assert!(!workspace.join(".git/index.lock").exists());
    assert_eq!(
        fixture.root.read("git-script.external/keep"),
        "outside checkout"
    );
    assert!(
        fixture
            .diagnostics
            .snapshot(&fixture.run_id)
            .diagnostics
            .is_empty()
    );
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}

#[tokio::test]
async fn exhausted_checkout_preserves_redacted_operator_diagnostics() {
    let fixture = CheckoutFixture::new(
        r#"case " $* " in
  *" fetch "*)
    /usr/bin/printf 'upstream status body\n'
    /usr/bin/printf '%05000d\n' 0
    /usr/bin/printf 'unfamiliar fetch failure: %s\n%s\n' "$GH_TOKEN" "$*" >&2
    exit 42 ;;
esac"#,
    )
    .await;
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::SourceCheckout)
    ));
    assert_eq!(fixture.root.read("git-script.attempts"), "xxx");
    let snapshot = fixture.diagnostics.snapshot(&fixture.run_id);
    assert_eq!(snapshot.diagnostics.len(), 1);
    let diagnostic = &snapshot.diagnostics[0];
    assert_eq!(diagnostic.operation, "source.checkout");
    assert_eq!(diagnostic.exit_status, Some(42));
    assert!(diagnostic.stdout.starts_with("upstream status body\n"));
    assert!(diagnostic.stdout_truncated);
    assert!(!diagnostic.stderr.contains("upstream status body"));
    assert!(
        diagnostic
            .stderr
            .contains("unfamiliar fetch failure: [REDACTED]")
    );
    assert!(diagnostic.stderr.contains("fetch"));
    assert!(!diagnostic.stderr.contains(CHECKOUT_TOKEN));
    assert!(
        !diagnostic
            .stderr
            .contains(&encode_basic_credential(CHECKOUT_TOKEN))
    );
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
    assert_eq!(
        fixture.root.read("git-script.external/keep"),
        "outside checkout"
    );
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::Runtime)
    ));
}

#[tokio::test]
async fn wrong_checkout_revision_never_exposes_a_runner() {
    let fixture = CheckoutFixture::new(
        r#"case " $* " in
  *" rev-parse HEAD "*)
    /usr/bin/printf '0000000000000000000000000000000000000000\n'
    exit 0 ;;
esac"#,
    )
    .await;
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::SourceCheckout)
    ));
    assert_eq!(fixture.root.read("git-script.attempts"), "x");
    let diagnostics = fixture.diagnostics.snapshot(&fixture.run_id).diagnostics;
    assert!(diagnostics[0].stderr.contains("checkout revision mismatch"));
    assert!(
        diagnostics[0]
            .stderr
            .contains(fixture.admitted.source.revision.as_str())
    );
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
}
