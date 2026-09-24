use std::future::Future;
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
    serve_prepared(server, listener, shutdown).await?;
    Ok(())
}

async fn serve_prepared<F>(
    server: Arc<NativeV2TargetServer>,
    listener: TcpListener,
    shutdown: F,
) -> Result<(), TargetServeError>
where
    F: Future<Output = ()>,
{
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
#[path = "serve/tests.rs"]
mod tests;
