use std::sync::Arc;

use openengine_cluster_server::identity::{
    BindingAttributes, ConnectionIdentity, ConnectionIdentityConfig, PrincipalId, TenantId,
};
use thiserror::Error;
use tokio::net::TcpListener;
use url::Url;
use zeroshot_engine::native_v2_cli::TargetServe;
use zeroshot_engine::native_v2_hosting::{
    ProductionHostingConfig, ProductionHostingError, build_production_target_authority,
};
use zeroshot_engine::native_v2_target_authority::{
    NativeV2TargetServer, OECP_PATH, TargetAuthorityError, TargetBootstrapKey,
};

use super::contract::normalize_origin;

#[derive(Debug, Error)]
pub enum TargetServeError {
    #[error(transparent)]
    Hosting(#[from] ProductionHostingError),
    #[error(transparent)]
    Authority(#[from] TargetAuthorityError),
    #[error("direct target server I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid direct target public origin: {0}")]
    InvalidOrigin(String),
    #[cfg(feature = "ui")]
    #[error(transparent)]
    Ui(#[from] zeroshot_engine::native_v2_cli::NativeV2CliError),
}

pub async fn serve_direct_target(config: TargetServe) -> Result<(), TargetServeError> {
    let shutdown = shutdown_signal()?;
    tokio::pin!(shutdown);
    let public_origin = normalize_origin(&config.public_origin)
        .map_err(|error| TargetServeError::InvalidOrigin(error.to_string()))?;
    let (server, listener) = tokio::select! {
        biased;
        () = &mut shutdown => return Ok(()),
        prepared = prepare_server(&config, &public_origin) => prepared?,
    };
    eprintln!(
        "Zeroshot direct target listening on {} as {}",
        config.listen, public_origin
    );
    #[cfg(feature = "ui")]
    if config.bootstrap_key_file.is_none() {
        eprintln!("Zeroshot UI: {public_origin}/ui/");
    }
    server.serve_until(listener, shutdown).await?;
    Ok(())
}

async fn prepare_server(
    config: &TargetServe,
    public_origin: &str,
) -> Result<(Arc<NativeV2TargetServer>, TcpListener), TargetServeError> {
    let bootstrap_key = config
        .bootstrap_key_file
        .as_deref()
        .map(TargetBootstrapKey::load_and_unlink)
        .transpose()?;
    let listener = TcpListener::bind(config.listen).await?;
    let endpoint = oecp_endpoint(public_origin)?;
    let hosting = ProductionHostingConfig {
        storage_root: config.storage.clone(),
        ..ProductionHostingConfig::default()
    };
    let target = Arc::new(build_production_target_authority(hosting).await?);
    // Target ownership, including recovery of interrupted runs, is established before any UI
    // reader can observe the ledger. UI connections never create a second controller.
    let controller = target.controller().await?;
    #[cfg(not(feature = "ui"))]
    let _ = controller;
    let server = match bootstrap_key {
        Some(key) => NativeV2TargetServer::new_private(target, direct_identity(), endpoint, key)?,
        None => {
            let server = NativeV2TargetServer::new_direct(target, direct_identity(), endpoint)?;
            #[cfg(feature = "ui")]
            let server = server.with_ui(zeroshot_engine::profile_ui::UiService::for_target(
                std::fs::canonicalize(&config.storage)?,
                public_origin,
                controller.observations(),
            )?)?;
            server
        }
    };
    Ok((Arc::new(server), listener))
}

#[cfg(unix)]
fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
}

#[cfg(not(unix))]
fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}

fn oecp_endpoint(origin: &str) -> Result<String, TargetAuthorityError> {
    let mut endpoint = Url::parse(origin)
        .map_err(|_| TargetAuthorityError::invalid("public target origin is invalid"))?;
    let scheme = match endpoint.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => {
            return Err(TargetAuthorityError::invalid(
                "public target origin is invalid",
            ));
        }
    };
    endpoint
        .set_scheme(scheme)
        .map_err(|()| TargetAuthorityError::invalid("public target origin is invalid"))?;
    endpoint.set_path(OECP_PATH);
    Ok(endpoint.into())
}

fn direct_identity() -> ConnectionIdentity {
    ConnectionIdentity::new(ConnectionIdentityConfig {
        principal: PrincipalId::new("direct-target"),
        tenant: TenantId::new("direct-target"),
        issued_at_ms: None,
        expires_at_ms: u64::MAX,
        binding_attributes: BindingAttributes::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

    struct Storage(std::path::PathBuf);

    impl Drop for Storage {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn relative_storage_has_an_initialized_ledger_before_accepting_connections() {
        let storage = Storage(
            std::path::PathBuf::from("target")
                .join(format!("target-serve-{}", uuid::Uuid::now_v7())),
        );
        let config = TargetServe {
            listen: "127.0.0.1:0".parse().assert_value(),
            public_origin: "http://127.0.0.1:8080".to_owned(),
            storage: storage.0.clone(),
            bootstrap_key_file: None,
        };
        let (server, listener) = prepare_server(&config, &config.public_origin)
            .await
            .assert_value();
        assert!(storage.0.join("runs.sqlite3").is_file());
        let ledger = zeroshot_engine::v2_run_ledger::sqlite::SqliteRunLedger::open_read_only(
            storage.0.join("runs.sqlite3"),
        )
        .assert_value();
        use zeroshot_engine::v2_run_ledger::RunLedger;
        assert!(ledger.list().await.assert_value().is_empty());
        drop(ledger);
        drop(listener);
        drop(server);
    }

    #[test]
    fn target_serve_derives_only_same_authority_websocket_endpoints() {
        assert_eq!(
            oecp_endpoint("http://127.0.0.1:8080").assert_value(),
            "ws://127.0.0.1:8080/native-v2/oecp"
        );
        assert_eq!(
            oecp_endpoint("https://target.example").assert_value(),
            "wss://target.example/native-v2/oecp"
        );
        assert_eq!(
            oecp_endpoint("http://[::1]:8080").assert_value(),
            "ws://[::1]:8080/native-v2/oecp"
        );
        for invalid in [
            "not a URL",
            "ftp://target.example",
            "ws://target.example",
            "file:///target",
        ] {
            assert_eq!(
                oecp_endpoint(invalid).assert_error().to_string(),
                "public target origin is invalid"
            );
        }

        let identity = direct_identity();
        assert_eq!(identity.principal().as_str(), "direct-target");
        assert_eq!(identity.tenant().as_str(), "direct-target");
        assert_eq!(identity.issued_at_ms(), None);
        assert_eq!(identity.expires_at_ms(), u64::MAX);
        assert!(identity.binding_attributes().iter().next().is_none());
    }

    #[tokio::test]
    async fn direct_serve_rejects_an_invalid_public_origin_before_preparing_storage() {
        let storage =
            openengine_cluster_testkit::TemporaryDirectory::for_test("target-serve-invalid-origin");
        for (case, origin) in [
            ("non-loopback-http", "http://target.example"),
            ("credentials", "https://user@target.example"),
            ("path", "https://target.example/private"),
            ("query", "https://target.example?token=private"),
        ] {
            let path = storage.path(case);
            let error = serve_direct_target(TargetServe {
                listen: "127.0.0.1:0".parse().assert_value(),
                public_origin: origin.to_owned(),
                storage: path.clone(),
                bootstrap_key_file: None,
            })
            .await
            .assert_error();
            assert!(matches!(error, TargetServeError::InvalidOrigin(_)));
            assert!(!path.exists(), "invalid origin prepared storage for {case}");
        }
    }
}
