//! Immutable pre-workspace main and v17.1.1 controls; pinned sources stay unmodified.
#[cfg(test)]
mod pre_workspace_main;
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
            panic!("initialization must fail before run payload decoding: {error}");
        };
        assert!(
            error
                .message
                .contains("upgrade the Zeroshot client and server together")
        );
        assert_eq!(error.code, released_protocol::APPLICATION_ERROR);
        let data = error.data.expect("domain error");
        assert_eq!(data.code, "UNSUPPORTED_PROTOCOL_VERSION");
        assert_eq!(
            data.details,
            Some(json!({
                "requestedProtocolVersion": "openengine.cluster/v2",
                "supportedProtocolVersion": PROTOCOL_VERSION,
            }))
        );
    }

    #[tokio::test]
    async fn released_client_is_rejected_at_ndjson_and_websocket_initialization() {
        assert_eq!(released_protocol::PROTOCOL_VERSION, "openengine.cluster/v2");
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
        rejected(
            client
                .initialize()
                .await
                .expect_err("incompatible released client"),
        );
        drop(transport);
        current::await_ndjson_shutdown(server).await;

        let (socket, server) = current::spawn_websocket(backend()).await;
        let transport = released_client::WebSocketTransport::new(socket);
        let client = released_client::ClusterClient::new(&transport);
        rejected(
            client
                .initialize()
                .await
                .expect_err("incompatible released client"),
        );
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
                "supportedProtocolVersion": "openengine.cluster/v2",
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

    #[test]
    fn released_continuation_record_reads_with_its_original_identity() {
        let prior = released_protocol::RunVector {
            id: released_protocol::VectorId::new("continuation-1").unwrap(),
            run_id: released_protocol::RunId::new("successor"),
            created_at: released_protocol::UnixTimestampMillis::new(42).unwrap(),
            content: released_protocol::RunVectorContent::Continuation {
                resumed_from: released_protocol::RunId::new("root"),
            },
        };
        let stored = serde_json::to_value(prior).unwrap();
        let current: openengine_cluster_protocol::RunVector =
            serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(serde_json::to_value(&current).unwrap(), stored);
        assert!(matches!(
            current.content,
            openengine_cluster_protocol::RunVectorContent::Continuation {
                checkpoint_id: None,
                ..
            }
        ));
    }
}
