//! Typed lifecycle parameter constructors for deterministic fixtures.

use openengine_cluster_protocol::{
    Generation, IdempotencyKey, RetryParams, StopMode, StopParams, TurnFailureKind, UpdateParams,
};
use openengine_cluster_server::lifecycle::{FailedCompletion, LeaseId};

#[must_use]
pub fn suspend(generation: u64, key: &str) -> UpdateParams {
    update_suspension(true, generation, key)
}

#[must_use]
pub fn resume(generation: u64, key: &str) -> UpdateParams {
    update_suspension(false, generation, key)
}

fn update_suspension(suspended: bool, generation: u64, key: &str) -> UpdateParams {
    UpdateParams {
        labels: None,
        log_level: None,
        suspended: Some(suspended),
        if_generation: fixture_generation(generation),
        idempotency_key: fixture_key(key),
    }
}

#[must_use]
pub fn stop(mode: StopMode, generation: u64, key: &str) -> StopParams {
    StopParams {
        mode,
        if_generation: fixture_generation(generation),
        idempotency_key: fixture_key(key),
    }
}

#[must_use]
pub fn retry(generation: u64, key: &str) -> RetryParams {
    RetryParams {
        if_generation: fixture_generation(generation),
        idempotency_key: fixture_key(key),
    }
}

#[must_use]
pub fn fail(kind: TurnFailureKind, lease_id: &str) -> FailedCompletion {
    FailedCompletion {
        lease_id: LeaseId::new(lease_id),
        kind,
    }
}

fn fixture_generation(value: u64) -> Generation {
    Generation::new(value).expect("fixture generation is in range")
}

fn fixture_key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("fixture key is valid")
}
