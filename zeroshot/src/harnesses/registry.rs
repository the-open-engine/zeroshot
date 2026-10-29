use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use crate::active_memory::hooks::ActiveMemoryHooks;
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
use crate::native_v2_contract::{RuntimeLane, RuntimePlan};
use super::{HarnessAdapter, HarnessBuildContext, HarnessConfig, HarnessError, ProviderAccessContract};

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSource {
    Core,
    Contributed,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct HarnessDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub providers: &'static [&'static str],
    pub executable: &'static str,
    pub source: HarnessSource,
}

pub struct HarnessRegistration {
    pub descriptor: HarnessDescriptor,
    pub build: fn(
        HarnessBuildContext,
        Option<Arc<ActiveMemoryHooks>>,
    ) -> Result<HarnessAdapter, HarnessError>,
    pub(crate) access: fn(&str) -> Result<ProviderAccessContract, HarnessError>,
    pub(crate) normalize_local: fn(&mut LocalHarnessEnvironment, &Path) -> Result<(), HarnessError>,
}

/// One enabled catalog supplies validation, authoring, discovery and adapter construction.
pub struct HarnessRegistry {
    entries: BTreeMap<&'static str, RegisteredHarness>,
}

struct RegisteredHarness {
    registration: HarnessRegistration,
    lanes: Vec<RuntimeLane>,
}

fn validated_lanes(descriptor: &HarnessDescriptor) -> Result<Vec<RuntimeLane>, HarnessError> {
    if descriptor.providers.is_empty()
        || descriptor.label.is_empty()
        || descriptor.label.len() > 256
        || descriptor.label.chars().any(char::is_control)
        || descriptor.executable.trim().is_empty()
        || descriptor.executable.chars().any(char::is_control)
    {
        return Err(HarnessError::Configuration(
            "harness descriptor is incomplete or invalid".to_owned(),
        ));
    }
    let lanes = descriptor
        .providers
        .iter()
        .map(|provider| RuntimeLane::new(descriptor.id, *provider))
        .collect::<Result<Vec<_>, _>>()?;
    if lanes.iter().collect::<BTreeSet<_>>().len() != lanes.len() {
        return Err(HarnessError::Configuration(
            "harness descriptor repeats a provider".to_owned(),
        ));
    }
    Ok(lanes)
}

impl HarnessRegistry {
    #[must_use]
    pub fn core() -> Self {
        let mut registry = Self {
            entries: BTreeMap::new(),
        };
        super::adapters::core::register(&mut registry);
        registry
    }

    #[must_use]
    pub fn local() -> Self {
        #[allow(unused_mut)]
        let mut registry = Self::core();
        #[cfg(feature = "contributed-harnesses")]
        super::adapters::contributed::register(&mut registry);
        registry
    }

    pub fn register(&mut self, registration: HarnessRegistration) -> Result<(), HarnessError> {
        let id = registration.descriptor.id;
        if self.entries.contains_key(id) {
            return Err(HarnessError::Duplicate(id.to_owned()));
        }
        let lanes = validated_lanes(&registration.descriptor)?;
        self.entries.insert(
            id,
            RegisteredHarness {
                registration,
                lanes,
            },
        );
        Ok(())
    }

    pub fn descriptor(&self, lane: &RuntimeLane) -> Result<&HarnessDescriptor, HarnessError> {
        let entry = self.entry(lane)?;
        Ok(&entry.descriptor)
    }

    fn entry(&self, lane: &RuntimeLane) -> Result<&HarnessRegistration, HarnessError> {
        let registered = self
            .entries
            .get(lane.harness_name())
            .ok_or_else(|| HarnessError::UnknownHarness(lane.harness_name().to_owned()))?;
        let entry = &registered.registration;
        if !entry.descriptor.providers.contains(&lane.provider_name()) {
            return Err(HarnessError::UnsupportedProvider(lane.clone()));
        }
        Ok(entry)
    }

    pub fn validate_runtime(&self, runtime: &RuntimePlan) -> Result<(), HarnessError> {
        self.entry(&runtime.lane())?;
        for lane in runtime.lanes() {
            self.entry(&lane)?;
        }
        Ok(())
    }

    pub fn configure(&self, context: HarnessBuildContext) -> Result<HarnessConfig, HarnessError> {
        let entry = self.entry(&context.lane)?;
        Ok(HarnessConfig {
            build: entry.build,
            lane: context.lane.clone(),
            context,
        })
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &HarnessDescriptor> {
        self.entries
            .values()
            .map(|entry| &entry.registration.descriptor)
    }

    pub fn lanes(&self) -> Vec<RuntimeLane> {
        self.entries
            .values()
            .flat_map(|entry| entry.lanes.iter().cloned())
            .collect()
    }

    pub(crate) fn access(
        &self,
        lane: &RuntimeLane,
    ) -> Result<ProviderAccessContract, HarnessError> {
        (self.entry(lane)?.access)(lane.provider_name())
    }

    pub(crate) fn normalize_local(
        &self,
        environment: &mut LocalHarnessEnvironment,
        directory: &Path,
    ) -> Result<(), HarnessError> {
        for entry in self.entries.values() {
            (entry.registration.normalize_local)(environment, directory)?;
        }
        Ok(())
    }
}
