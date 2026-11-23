//! Immutable v16.0.0 initialization controls. No released Rust source is copied or modified.
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use openengine_cluster_client::{ClusterClient, ClientError, NdjsonTransport, WebSocketTransport};
    use openengine_cluster_server::watch::fixtures::{self as current, FixtureBackend, FixtureStore};
    use openengine_cluster_protocol::{RunId, PROTOCOL_VERSION};
    use released_server::watch::fixtures as old;
    use serde_json::json;

    fn rejected(error: released_client::ClientError) {
        let released_client::ClientError::Rpc(error) = error else {
            panic!("initialization must fail before timing payload decoding: {error}");
        };
        assert_eq!(error.message, "Unsupported protocol version");
        assert_eq!(error.code, released_protocol::APPLICATION_ERROR);
        let data = error.data.expect("domain error");
        assert_eq!(data.code, "UNSUPPORTED_PROTOCOL_VERSION");
        assert_eq!(
            data.details,
            Some(json!({
                "requestedProtocolVersion": "openengine.cluster/v1",
                "supportedProtocolVersion": PROTOCOL_VERSION,
            }))
        );
    }

    #[tokio::test]
    async fn exact_v16_client_is_rejected_at_ndjson_and_websocket_initialization() {
        assert_eq!(released_protocol::PROTOCOL_VERSION, "openengine.cluster/v1");
        let backend = || {
            FixtureBackend::new(Arc::new(FixtureStore::new(
                RunId::new("unused"),
                Vec::new(),
                8,
            )))
        };
        let (write, read, server) = current::spawn_ndjson(backend());
        let transport = released_client::NdjsonTransport::new(read, write);
        let client = released_client::ClusterClient::new(&transport);
        // The released client propagates initialization failure before the first observe call.
        let result = async {
            client.initialize().await?;
            client
                .run_list(released_protocol::RunListParams::default())
                .await?;
            Ok::<_, released_client::ClientError>(())
        }
        .await;
        rejected(result.expect_err("incompatible released client"));
        drop(transport);
        current::await_ndjson_shutdown(server).await;

        let (socket, server) = current::spawn_websocket(backend()).await;
        let transport = released_client::WebSocketTransport::new(socket);
        let client = released_client::ClusterClient::new(&transport);
        let result = async {
            client.initialize().await?;
            client
                .run_list(released_protocol::RunListParams::default())
                .await?;
            Ok::<_, released_client::ClientError>(())
        }
        .await;
        rejected(result.expect_err("incompatible released client"));
        drop(transport);
        current::await_websocket_shutdown(server).await;
    }

    fn upgrade(error: ClientError) {
        assert!(
            error
                .to_string()
                .contains("upgrade the Zeroshot client and server together")
        );
        let ClientError::Rpc(error) = error else {
            panic!("expected protocol rejection");
        };
        let data = error.data.expect("domain error");
        assert_eq!(data.code, "UNSUPPORTED_PROTOCOL_VERSION");
        assert_eq!(
            data.details,
            Some(json!({
                "requestedProtocolVersion": PROTOCOL_VERSION,
                "supportedProtocolVersion": "openengine.cluster/v1",
            }))
        );
    }

    #[tokio::test]
    async fn current_client_stops_at_released_server_initialization_on_both_transports() {
        let backend = || {
            old::FixtureBackend::new(Arc::new(old::FixtureStore::new(
                released_protocol::RunId::new("unused"),
                Vec::new(),
                8,
            )))
        };
        let (write, read, server) = old::spawn_ndjson(backend());
        let transport = NdjsonTransport::new(read, write);
        upgrade(
            ClusterClient::new(&transport)
                .initialize()
                .await
                .expect_err("old server"),
        );
        drop(transport);
        old::await_ndjson_shutdown(server).await;
        let (socket, server) = old::spawn_websocket(backend()).await;
        let transport = WebSocketTransport::new(socket);
        upgrade(
            ClusterClient::new(&transport)
                .initialize()
                .await
                .expect_err("old server"),
        );
        drop(transport);
        old::await_websocket_shutdown(server).await;
    }
}

#[cfg(test)]
mod producer;
