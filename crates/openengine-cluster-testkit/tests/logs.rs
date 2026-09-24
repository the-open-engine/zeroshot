use std::sync::atomic::Ordering;
use std::sync::Arc;

use openengine_cluster_protocol::{
    BoundedLogMessage, BoundedLogTarget, GraphProfileSet, InitializeParams, LogEventNotification,
    LogLevel, LogRecord, ServerCapabilities, PROTOCOL_VERSION,
};
use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::logs::LogStore;
use openengine_cluster_server::{ClusterBackend, ConnectionContext};
use openengine_cluster_testkit::admission::{InMemoryAdmissionStore, ScriptedVerifier};
use openengine_cluster_testkit::capability_vectors::assert_logs_capability;
use openengine_cluster_testkit::logs::InMemoryLogStore;

#[path = "schema_support/mod.rs"]
mod schema_support;
use schema_support::find_schema;

fn initialize_params() -> InitializeParams {
    InitializeParams {
        protocol_version: PROTOCOL_VERSION.to_owned(),
    }
}

fn record(message: &str) -> LogRecord {
    LogRecord {
        level: LogLevel::Info,
        target: BoundedLogTarget::new("testkit").assert_value(),
        message: BoundedLogMessage::new(message).assert_value(),
    }
}

#[tokio::test]
async fn in_memory_store_drops_cancelled_and_overflowed_subscribers_independently() {
    let store = InMemoryLogStore::new();
    let mut slow = store.subscribe(0).await;
    let cancelled = store.subscribe(1).await;
    drop(cancelled.receiver);

    store.publish(record("first")).await;
    assert!(!slow.overflowed.load(Ordering::Acquire));
    store.publish(record("second")).await;
    assert!(slow.overflowed.load(Ordering::Acquire));

    assert_eq!(slow.receiver.recv().await, Some(record("first")));
    store.publish(record("after-overflow")).await;
    assert!(matches!(
        slow.receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn logs_capability_is_true_only_when_a_log_store_is_injected() {
    let plain = AdmissionCoordinator::new(
        ScriptedVerifier::new(vec![]),
        InMemoryAdmissionStore::default(),
    );
    let plain_capabilities = plain
        .initialize(&ConnectionContext::default(), initialize_params())
        .await
        .assert_value()
        .capabilities;
    assert_logs_capability(&plain_capabilities, false);

    let with_store = AdmissionCoordinator::new(
        ScriptedVerifier::new(vec![]),
        InMemoryAdmissionStore::default(),
    )
    .with_log_store(Arc::new(InMemoryLogStore::default()) as Arc<dyn LogStore>);
    let with_store_capabilities = with_store
        .initialize(&ConnectionContext::default(), initialize_params())
        .await
        .assert_value()
        .capabilities;
    assert_logs_capability(&with_store_capabilities, true);
}

#[test]
fn logs_capability_vector_matches_server_capabilities() {
    let disabled = ServerCapabilities {
        graph_profiles: GraphProfileSet::new(vec![]).assert_value(),
        logs: false,
        agent_attach: false,
    };
    assert_logs_capability(&disabled, false);

    let enabled = ServerCapabilities {
        graph_profiles: GraphProfileSet::new(vec![]).assert_value(),
        logs: true,
        agent_attach: false,
    };
    assert_logs_capability(&enabled, true);
}

#[tokio::test]
async fn generated_logs_goldens_validate_against_the_published_schema() {
    let artifacts = openengine_cluster_testkit::artifacts::generate_artifacts().await;
    let schema = find_schema(&artifacts);
    let mut event_schema = schema
        .assert_key("$defs")
        .assert_key("LogEventNotification")
        .clone();
    event_schema
        .as_object_mut()
        .assert_value()
        .insert("$defs".to_owned(), schema.assert_key("$defs").clone());
    let event_validator = jsonschema::validator_for(&event_schema).assert_value();

    let session = artifacts
        .iter()
        .find(|artifact| {
            artifact
                .relative_path
                .ends_with("/goldens/logs-session.json")
        })
        .assert_value();
    let notifications: Vec<LogEventNotification> =
        serde_json::from_slice(&session.bytes).assert_value();
    assert!(!notifications.is_empty());
    for notification in &notifications {
        let value = serde_json::to_value(notification).assert_value();
        assert!(
            event_validator.is_valid(&value),
            "generated log event notification failed schema validation: {value}"
        );
    }
}

use openengine_cluster_testkit::assertions::AssertValue;

use openengine_cluster_testkit::assertions::JsonAt;
