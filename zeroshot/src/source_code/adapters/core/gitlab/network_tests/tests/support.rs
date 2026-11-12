use super::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use rustls::pki_types::PrivateKeyDer;

pub(super) struct TlsFixture {
    pub inner: Fixture,
    pub address: SocketAddr,
    pub ca_file: std::path::PathBuf,
    base: SourceForgeBaseUrl,
    server: tokio::task::JoinHandle<()>,
    _root: tempfile::TempDir,
}

impl Drop for TlsFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl TlsFixture {
    pub fn prepared(
        &self,
        mode: crate::native_v2_delivery::DeliveryMode,
        token: &str,
    ) -> crate::native_v2_local::PreparedLocalRun {
        use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
        use crate::native_v2_supervisor::RunEnvironment;
        let mut node = git_delivery_node();
        node["worker"] = json!(mode.worker_ref());
        node["output"] = serde_json::to_value(
            crate::native_v2_delivery::contract::delivery_result_schema(mode).unwrap(),
        )
        .unwrap();
        node["signals"] =
            json!({"delivery": crate::native_v2_delivery::delivery_signal_labels(mode).unwrap()});
        let submission = serde_json::from_value::<openengine_cluster_protocol::RunSubmission>(json!({
            "title": "Forge preflight", "graph": full_graph(vec![node, success_node()]), "initialInput": null,
            "runtime": {"harness":"codex", "provider":"openai", "size":"host", "nodes": {"deliver":{"kind":"git_delivery", "connections":{"forge":["GITLAB_TOKEN"]}}}},
            "source": {"repository":REPOSITORY,"branch":"main","revision":BASE,"remote":{"url":format!("{}/{REPOSITORY}.git", self.base),"forge":{"provider":"source.gitlab","repository":REPOSITORY,"baseUrl":self.base}}},
            "submissionKey":"forge-preflight",
        })).unwrap();
        let environment = RunEnvironment::exact_for_submission(
            &submission,
            serde_json::from_value(json!({"forge":{"GITLAB_TOKEN":token}})).unwrap(),
        )
        .unwrap();
        let mut native_environment: BTreeMap<_, _> = std::env::vars().collect();
        for name in crate::source_code_provider::adapters::core::network::NETWORK_FIELDS {
            native_environment.remove(*name);
        }
        native_environment.extend(self.ca_environment());
        crate::native_v2_local::PreparedLocalRun {
            run_id: openengine_cluster_protocol::RunId::new("forge-preflight"),
            delivery_run_id: openengine_cluster_protocol::RunId::new("forge-preflight"),
            submission,
            environment,
            github_token: None,
            github_token_file: None,
            delivery_validator: None,
            git_credentials: None,
            workspace: self.ca_file.parent().unwrap().into(),
            local_branch: Some("feature/preflight".into()),
            invoking_workspace: None,
            request_digest: None,
            workspace_revision: None,
            native_environment,
        }
    }

    pub async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
        let cert = params.signed_by(&key, &ca, &ca_key).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone(), ca.der().clone()],
            PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let base = SourceForgeBaseUrl::new(format!("https://{address}/service")).unwrap();
        let inner = Fixture::new().await;
        let state = inner.state.clone();
        let origin = base.to_string();
        let server = tokio::spawn(http_fixture::serve_tls(
            listener,
            tokio_rustls::TlsAcceptor::from(Arc::new(config)),
            move |request| {
                if request.uri.path() == "/api/v3/repos/acme/project" {
                    assert_eq!(request.headers["authorization"], "token test-private-token");
                    return Json(json!({"full_name":"acme/project"}))
                        .into_response()
                        .header("X-GitHub-Enterprise-Version", "enterprise-server@3.22.2");
                }
                if request.uri.path().ends_with("/api/graphql") {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    if body["query"]
                        .as_str()
                        .unwrap_or("")
                        .contains("ZeroshotForgeAccess")
                    {
                        return access_response(&state, request, &origin);
                    }
                }
                handler(&state, request).replace_origin(&origin)
            },
        ));
        let ca_file = root.path().join("private-ca.pem");
        std::fs::write(&ca_file, ca.pem()).unwrap();
        Self {
            inner,
            address,
            ca_file,
            base,
            server,
            _root: root,
        }
    }

    pub fn ca_environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "SSL_CERT_FILE".into(),
            self.ca_file.to_string_lossy().into_owned(),
        )])
    }

    pub fn authority(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> Arc<dyn SourceDeliveryAuthority> {
        use crate::source_code_provider::adapters::core::{
            SourceDeliveryFactoryRequest, delivery_authority,
        };
        let source = serde_json::from_value::<openengine_cluster_protocol::ResolvedSource>(json!({
            "repository": REPOSITORY, "branch": "main", "revision": BASE,
            "remote": {"url": format!("{}/{REPOSITORY}.git", self.base), "forge": {
                "provider": "source.gitlab", "repository": REPOSITORY, "baseUrl": self.base,
            }},
        }))
        .unwrap();
        let root = self.ca_file.parent().unwrap();
        delivery_authority(
            SourceDeliveryFactoryRequest {
                source: &source,
                workspace: root,
                home_directory: root,
                private_directory: root,
                git_program: std::path::Path::new("git"),
                gh_program: std::path::Path::new("gh"),
                git_identity: None,
                local_environment: Some(environment.clone()),
                required_capabilities: &[SourceCapability::PullRequest, SourceCapability::Merge],
                operator_diagnostics: None,
            },
            None,
            None,
        )
        .unwrap()
    }

    pub fn calls(&self) -> Vec<(String, String, Value)> {
        self.inner.state.lock().unwrap().calls.clone()
    }
}

fn access_response(
    state: &Mutex<StateData>,
    request: http_fixture::Request,
    origin: &str,
) -> Response {
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let mut state = state.lock().unwrap();
    state.calls.push((
        request.method.to_string(),
        request.uri.path().into(),
        body.clone(),
    ));
    if request.headers.get("authorization").unwrap() != "Bearer test-private-token" {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message":"token rejected"})),
        )
            .into_response();
    }
    let project = (body["variables"]["project"] == REPOSITORY)
        .then(|| json!({"fullPath": REPOSITORY, "webUrl": format!("{origin}/{REPOSITORY}")}));
    Json(json!({"data":{"currentUser":{"id":"gid://gitlab/User/42"},"metadata":{"version":state.version},"project":project}})).into_response()
}

pub(super) struct ProxyFixture {
    address: SocketAddr,
    calls: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ProxyFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl ProxyFixture {
    pub async fn new(target: SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut client, _) = listener.accept().await.unwrap();
                let count = count.clone();
                tokio::spawn(async move {
                    let mut header = Vec::new();
                    while !header.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        if client.read_exact(&mut byte).await.is_err() || header.len() > 8192 {
                            return;
                        }
                        header.push(byte[0]);
                    }
                    assert!(
                        String::from_utf8_lossy(&header).starts_with(&format!("CONNECT {target} "))
                    );
                    assert!(
                        String::from_utf8_lossy(&header)
                            .to_ascii_lowercase()
                            .contains(
                                "proxy-authorization: basic cHJveHktdXNlcjpwcm94eS1wYXNzd29yZA=="
                                    .to_ascii_lowercase()
                                    .as_str()
                            )
                    );
                    count.fetch_add(1, Ordering::SeqCst);
                    let mut upstream = tokio::net::TcpStream::connect(target).await.unwrap();
                    client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                });
            }
        });
        Self {
            address,
            calls,
            server,
        }
    }
    pub fn url(&self) -> String {
        format!("http://proxy-user:proxy-password@{}", self.address)
    }
    pub fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
