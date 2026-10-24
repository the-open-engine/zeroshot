use super::*;
use crate::native_v2_review::{PreparedReview, ReviewResolutionRequest};
use openengine_cluster_protocol::{
    COMMENT_DELIVERY_KIND, TASK_RESOLUTION_KIND, TARGET_TASK_RESOLUTION_PATH,
};

#[derive(Default)]
struct TaskFactory {
    host_owned: bool,
    resolved: AtomicUsize,
    controllers: AtomicUsize,
}

#[async_trait]
impl TargetControllerFactory for TaskFactory {
    fn supports_comment_delivery(&self) -> bool {
        true
    }
    fn supports_task_resolution(&self, _authentication: TargetAuthentication) -> bool {
        true
    }
    async fn resolve_task(
        &self,
        request: ReviewResolutionRequest,
        github_token: Option<String>,
    ) -> Result<PreparedReview, TargetAuthorityError> {
        let expected = if self.host_owned {
            None
        } else {
            Some("forge-token")
        };
        if github_token.as_deref() != expected {
            return Err(TargetAuthorityError::invalid(
                "explicit forge credential required",
            ));
        }
        self.resolved.fetch_add(1, Ordering::SeqCst);
        Ok(PreparedReview {
            initial_input: json!({"url":request.url,"context":{"title":"issue","body":"review this","reviewScope":{}}}),
            source: super::request().submission.source.unwrap(),
        })
    }
    async fn create(&self) -> Result<Arc<NativeV2CloudController>, TargetAuthorityError> {
        self.controllers.fetch_add(1, Ordering::SeqCst);
        test_controller().await
    }
    async fn submit(
        &self,
        _controller: &NativeV2CloudController,
        request: TargetRunRequest,
    ) -> Result<TargetRunReceipt, TargetAuthorityError> {
        Ok(TargetRunReceipt {
            run_id: request.run_id,
        })
    }
}

#[tokio::test]
async fn task_resolution_authenticates_before_effects_and_never_creates_controller() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let factory = Arc::new(TaskFactory {
        host_owned: true,
        ..Default::default()
    });
    let server = Arc::new(
        NativeV2TargetServer::new_hosted(
            Arc::new(NativeV2TargetAuthority::new(factory.clone())),
            Arc::new(FakeSessions),
            format!("ws://{address}{OECP_PATH}"),
        )
        .assert_value(),
    );
    let task = tokio::spawn(server.serve(listener));
    let encoded = serde_json::to_vec(&json!({"url":"https://github.com/acme/project/issues/17"}))
        .assert_value();
    assert_eq!(
        http(
            address,
            TestHttpRequest::body("POST", TARGET_TASK_RESOLUTION_PATH, None, &encoded)
        )
        .await
        .status,
        401
    );
    assert_eq!(factory.resolved.load(Ordering::SeqCst), 0);
    let response = http(
        address,
        TestHttpRequest::body(
            "POST",
            TARGET_TASK_RESOLUTION_PATH,
            Some("control-token"),
            &encoded,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_slice(&response.body).assert_value();
    assert_eq!(value["initialInput"]["context"]["title"], "issue");
    assert!(
        !String::from_utf8(response.body)
            .assert_value()
            .contains("forge-token")
    );
    assert_eq!(factory.controllers.load(Ordering::SeqCst), 0);
    let explicit_secret = serde_json::to_vec(&json!({"request":{"url":"https://github.com/acme/project/issues/17"},"githubToken":"forge-token"})).assert_value();
    assert_eq!(
        http(
            address,
            TestHttpRequest::body(
                "POST",
                TARGET_TASK_RESOLUTION_PATH,
                Some("control-token"),
                &explicit_secret
            )
        )
        .await
        .status,
        400
    );
    let discovery = target_discovery(address).await;
    assert_eq!(
        discovery.extensions.comment_delivery.assert_value().kind,
        COMMENT_DELIVERY_KIND
    );
    let resolution = discovery.extensions.task_resolution.assert_value();
    assert_eq!(resolution.kind, TASK_RESOLUTION_KIND);
    assert_eq!(resolution.resolve, TARGET_TASK_RESOLUTION_PATH);
    task.abort();
}

#[tokio::test]
async fn direct_resolution_accepts_only_explicit_credential_envelope() {
    let factory = Arc::new(TaskFactory::default());
    let (address, _, task) = direct_test_server(factory.clone()).await;
    let bare = serde_json::to_vec(&json!({"url":"https://github.com/acme/project/issues/17"}))
        .assert_value();
    assert_eq!(
        http(
            address,
            TestHttpRequest::body("POST", TARGET_TASK_RESOLUTION_PATH, None, &bare)
        )
        .await
        .status,
        400
    );
    let explicit = serde_json::to_vec(&json!({"request":{"url":"https://github.com/acme/project/issues/17"},"githubToken":"forge-token"})).assert_value();
    assert_eq!(
        http(
            address,
            TestHttpRequest::body("POST", TARGET_TASK_RESOLUTION_PATH, None, &explicit)
        )
        .await
        .status,
        200
    );
    assert_eq!(factory.resolved.load(Ordering::SeqCst), 1);
    assert_eq!(factory.controllers.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn production_resolution_requires_explicit_authority_and_omits_hosted_support() {
    let factory =
        crate::native_v2_hosting::ProductionTargetControllerFactory::new(Default::default());
    assert!(factory.supports_task_resolution(TargetAuthentication::None));
    assert!(factory.supports_task_resolution(TargetAuthentication::PrivateCapability));
    assert!(!factory.supports_task_resolution(TargetAuthentication::HostedOauth));
    let error = factory
        .resolve_task(
            ReviewResolutionRequest {
                url: "https://github.com/acme/project/issues/17".to_owned(),
                branch: None,
                revision: None,
            },
            None,
        )
        .await
        .expect_err("no ambient forge authority");
    assert_eq!(error.kind(), TargetAuthorityErrorKind::Invalid);
    assert!(error.message().contains("explicit GitHub credential"));
}

#[tokio::test]
async fn unsupported_target_omits_and_rejects_task_resolution() {
    let (address, _, task) = direct_test_server(Arc::new(FakeFactory::default())).await;
    let discovery = target_discovery(address).await;
    assert!(discovery.extensions.comment_delivery.is_none());
    assert!(discovery.extensions.task_resolution.is_none());
    assert_eq!(
        http(
            address,
            TestHttpRequest::body("POST", TARGET_TASK_RESOLUTION_PATH, None, b"{}")
        )
        .await
        .status,
        404
    );
    task.abort();
}
