use super::*;
use crate::native_v2_capsule::{CapsuleFilesystemSpec, prepare_capsule_filesystem};

#[tokio::test]
#[cfg(target_os = "linux")]
async fn root_copilot_uses_the_hosted_process_boundary() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only Copilot process gate skipped outside the hosted identity");
        return;
    }
    let directory = TestDirectory::new("copilot-hosted");
    let executable = directory.write_executable("copilot", SCRIPT);
    let workspace = directory.child("workspace");
    let runtime_home = directory.child("runtime");
    let process_pool = HostedProcessPool::new(31_602, 31_602, 32_600, 32_600).assert_value();
    prepare_capsule_filesystem(CapsuleFilesystemSpec {
        workspace: &workspace,
        runtime_home: &runtime_home,
        process_pool,
    })
    .assert_value();
    let values = BTreeMap::from([
        (auth::TOKEN.to_owned(), "gho_fake-secret".to_owned()),
        ("TEST_MODE".to_owned(), "permission".to_owned()),
        (
            "CAPTURE_PATH".to_owned(),
            workspace.join("capture").display().to_string(),
        ),
    ]);
    let binding = binding(values.keys().cloned());
    let admitted = admitted(binding.clone(), INSTRUCTIONS).await;
    let adapter = Arc::new(CopilotAdapter::new(CopilotConfig {
        executable,
        workspace,
        runtime_home,
        local_user: None,
        base_environment: BTreeMap::new(),
        local_command_environment: BTreeMap::from([(
            "HOST_ONLY_SECRET".to_owned(),
            "must-not-reach-hosted-copilot".to_owned(),
        )]),
        search_path: "/usr/bin:/bin".to_owned(),
        process_pool,
    }));
    let runtime = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
    let fixture = Fixture {
        directory,
        runtime,
        binding,
        values,
        instructions: INSTRUCTIONS,
    };
    verified(complete(fixture.start(1).await).await.1);
    verified(complete(fixture.start(2).await).await.1);
}
