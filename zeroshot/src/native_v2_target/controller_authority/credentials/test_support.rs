use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;

#[derive(Default)]
pub struct MemoryCredentialStore {
    values: Mutex<BTreeMap<String, String>>,
}

pub struct UnavailableCredentialStore;

#[derive(Default)]
pub struct MemoryDeviceCodeNotifier {
    values: Mutex<Vec<(String, String)>>,
}

impl MemoryDeviceCodeNotifier {
    pub fn values(&self) -> Vec<(String, String)> {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl DeviceCodeNotifier for MemoryDeviceCodeNotifier {
    fn show(&self, verification_uri: &str, user_code: &str) {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((verification_uri.to_owned(), user_code.to_owned()));
    }
}

#[async_trait]
impl TargetCredentialStore for UnavailableCredentialStore {
    async fn remove(&self, _target_id: &str) -> Result<(), TargetAuthorityError> {
        Err(authority_error("test credential store unavailable"))
    }
    async fn prepare_for_login(
        &self,
        _target_id: &str,
    ) -> Result<CredentialStorePreparation, TargetAuthorityError> {
        Err(authority_error("test credential store unavailable"))
    }

    async fn get(&self, _target_id: &str) -> Result<Option<String>, TargetAuthorityError> {
        Err(authority_error("test credential store unavailable"))
    }

    async fn set(
        &self,
        _target_id: &str,
        _refresh_token: &str,
    ) -> Result<(), TargetAuthorityError> {
        Err(authority_error("test credential store unavailable"))
    }
}

#[async_trait]
impl TargetCredentialStore for MemoryCredentialStore {
    async fn remove(&self, target_id: &str) -> Result<(), TargetAuthorityError> {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(target_id);
        Ok(())
    }
    async fn get(&self, target_id: &str) -> Result<Option<String>, TargetAuthorityError> {
        Ok(self
            .values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(target_id)
            .cloned())
    }

    async fn set(&self, target_id: &str, refresh_token: &str) -> Result<(), TargetAuthorityError> {
        self.values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target_id.to_owned(), refresh_token.to_owned());
        Ok(())
    }
}
