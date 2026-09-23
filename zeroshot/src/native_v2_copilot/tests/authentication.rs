use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use crate::native_v2_runner::{
    EnvironmentRefreshError, ResolvedEnvironment, RuntimeEnvironmentRefresh,
    with_environment_refresh,
};

struct Refresh {
    environment: ResolvedEnvironment,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl RuntimeEnvironmentRefresh for Refresh {
    async fn refresh(&self) -> Result<ResolvedEnvironment, EnvironmentRefreshError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.environment.clone())
    }
}

fn expiry() -> String {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .assert_value()
        .as_secs()
        + 7200)
        .to_string()
}

async fn rejected_before_process(fixture: Fixture) {
    let (_, outcome) = complete(fixture.start(1).await).await;
    assert!(outcome.is_err());
    assert!(!fixture.directory.child("capture").exists());
}

#[tokio::test]
async fn credential_callback_refreshes_through_the_existing_resolver() {
    let fixture = Fixture::with_values(
        "refresh",
        BTreeMap::from([(auth::EXPIRES_AT.to_owned(), expiry())]),
    )
    .await;
    let mut request = fixture.request(1);
    let mut rotated = fixture.values.clone();
    rotated.insert(auth::TOKEN.to_owned(), "rotated-sensitive-value".to_owned());
    let environment = ResolvedEnvironment::exact(
        &fixture.binding,
        rotated
            .into_iter()
            .map(|(key, value)| (environment_name(&key), value))
            .collect(),
    )
    .assert_value();
    let refresh = Arc::new(Refresh {
        environment,
        calls: AtomicUsize::new(0),
    });
    request.environment = with_environment_refresh(request.environment, refresh.clone());
    verified(
        complete(fixture.runtime.start(request).await.assert_value())
            .await
            .1,
    );
    assert_eq!(refresh.calls.load(Ordering::Relaxed), 1);
    let capture = fixture.capture();
    let created = capture
        .iter()
        .find(|message| message["method"] == "session.create")
        .assert_value();
    assert!(created["params"].get("gitHubToken").is_none());
    assert_eq!(
        created["params"]["gitHubTokenProviderRegistrationId"],
        auth::REGISTRATION
    );
}

#[tokio::test]
async fn expiry_and_callback_identity_fail_closed() {
    let fixture = Fixture::with_values(
        "success",
        BTreeMap::from([(auth::EXPIRES_AT.to_owned(), "1".to_owned())]),
    )
    .await;
    let environment = fixture.request(1).environment;
    assert!(auth::result(&environment).is_err());
    for params in [
        json!({"registrationId":auth::REGISTRATION,"host":"github.com","reason":"initial"}),
        json!({"registrationId":auth::REGISTRATION,"host":"elsewhere.invalid","reason":"initial"}),
        json!({"registrationId":"other","host":"https://github.com","reason":"initial"}),
        json!({"registrationId":auth::REGISTRATION,"host":"https://github.com","sessionId":"other","reason":"initial"}),
    ] {
        assert!(
            auth::acquire(&params, &environment, "session")
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn declared_invalid_token_cannot_fall_back_to_native_identity() {
    let fixture = Fixture::with_local_provider(
        "declared_token",
        BTreeMap::from([(auth::TOKEN.to_owned(), "   ".to_owned())]),
        custom_provider_environment(),
    )
    .await;
    rejected_before_process(fixture).await;
}

#[tokio::test]
async fn dynamic_auth_accepts_the_configured_custom_github_host() {
    let fixture = Fixture::with_local_provider(
        "success",
        BTreeMap::from([
            (auth::TOKEN.to_owned(), "ghe-user-token".to_owned()),
            (auth::EXPIRES_AT.to_owned(), expiry()),
        ]),
        BTreeMap::from([("COPILOT_GH_HOST".to_owned(), "octocorp.ghe.com".to_owned())]),
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
}

#[tokio::test]
async fn reserved_provider_controls_are_never_passed_to_agent_processes() {
    for name in [
        "HOME",
        "COPILOT_HOME",
        "COPILOT_AUTO_UPDATE",
        "NODE_OPTIONS",
        "NODE_PATH",
        "BUN_OPTIONS",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "LD_PROFILE",
        "DYLD_INSERT_LIBRARIES",
        "OPENSSL_CONF",
        "OPENSSL_MODULES",
        "OPENSSL_ENGINES",
    ] {
        let fixture = Fixture::with_values(
            "success",
            BTreeMap::from([(name.to_owned(), "sentinel".to_owned())]),
        )
        .await;
        rejected_before_process(fixture).await;
    }
}

#[tokio::test]
async fn large_prompt_and_cumulative_rpc_output_do_not_deadlock_or_hit_a_total_limit() {
    let fixture = Fixture::new("duplex").await;
    let mut request = fixture.request(1);
    request.invocation.input = json!({"large": "x".repeat(2 * 1024 * 1024)});
    verified(
        complete(fixture.runtime.start(request).await.assert_value())
            .await
            .1,
    );
}

#[tokio::test]
#[ignore = "requires an explicitly authorized live Copilot user token and pinned CLI"]
async fn live_user_token_callback_authenticates_and_resumes() {
    let token = std::env::var("ZEROSHOT_COPILOT_TEST_TOKEN").assert_value();
    let executable = std::env::var_os("ZEROSHOT_COPILOT_TEST_EXECUTABLE").assert_value();
    // The non-expiring test OAuth token receives a finite lease to exercise callback transport.
    let fixture = Fixture::with_executable(
        TestDirectory::new("copilot-live-callback"),
        PathBuf::from(executable),
        BTreeMap::from([
            (auth::TOKEN.to_owned(), token),
            (auth::EXPIRES_AT.to_owned(), expiry()),
        ]),
        INSTRUCTIONS,
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
    verified(complete(fixture.start(2).await).await.1);
}
