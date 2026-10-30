use super::*;
use crate::native_v2_contract::{RuntimeLane, RuntimePlan};

fn registration(id: &'static str, providers: &'static [&'static str]) -> HarnessRegistration {
    HarnessRegistration {
        descriptor: HarnessDescriptor {
            id,
            label: "Fixture harness",
            providers,
            executable: "fixture-harness",
            source: HarnessSource::Contributed,
        },
        build: |_, _| panic!("metadata operations must not construct an adapter"),
        access: |_| panic!("metadata operations must not resolve provider access"),
        normalize_local: |_, _| panic!("metadata operations must not inspect local state"),
    }
}

#[test]
fn registration_validates_metadata_before_publishing_any_lane() {
    let mut registry = HarnessRegistry::core();
    let before = registry.lanes();
    for candidate in [
        registration("", &["provider"]),
        registration("invalid\nharness", &["provider"]),
        registration("fixture", &[""]),
        registration("fixture", &["invalid\nprovider"]),
        registration("fixture", &["provider", "provider"]),
        registration("fixture", &[]),
    ] {
        assert!(registry.register(candidate).is_err());
        assert_eq!(registry.lanes(), before);
    }
    let mut candidate = registration("fixture", &["provider"]);
    candidate.descriptor.executable = "";
    assert!(registry.register(candidate).is_err());
    let mut candidate = registration("fixture", &["provider"]);
    candidate.descriptor.label = "invalid\nlabel";
    assert!(registry.register(candidate).is_err());
    assert_eq!(registry.lanes(), before);
}

#[test]
fn registration_rejects_duplicate_ids_and_keeps_every_validated_lane() {
    let mut registry = HarnessRegistry::core();
    let initial = registry.lanes();
    registry
        .register(registration("fixture", &["private/one", "private:two"]))
        .unwrap();
    assert!(matches!(
        registry.register(registration("fixture", &["other"])),
        Err(HarnessError::Duplicate(_))
    ));
    let lanes = registry.lanes();
    assert_eq!(lanes.len(), initial.len() + 2);
    for lane in initial {
        assert!(lanes.contains(&lane));
    }
    for provider in ["private/one", "private:two"] {
        let lane = RuntimeLane::new("fixture", provider).unwrap();
        assert!(lanes.contains(&lane));
        assert_eq!(registry.descriptor(&lane).unwrap().id, "fixture");
    }
    assert!(matches!(
        registry.descriptor(&RuntimeLane::new("fixture", "missing").unwrap()),
        Err(HarnessError::UnsupportedProvider(_))
    ));
}

#[test]
fn runtime_validation_checks_default_and_each_node_override_without_effects() {
    let mut registry = HarnessRegistry::core();
    registry
        .register(registration("fixture", &["provider"]))
        .unwrap();
    let mut runtime = RuntimePlan::new(
        RuntimeLane::new("fixture", "provider").unwrap(),
        crate::native_v2_contract::RunSize::Small,
        Default::default(),
    );
    registry.validate_runtime(&runtime).unwrap();
    let worker = crate::native_v2_contract::NodeRuntimeBinding::Agent {
        lane: Some(RuntimeLane::new("unregistered", "provider").unwrap()),
        model: openengine_cluster_protocol::ModelId::new("opaque-model").unwrap(),
        effort: None,
        context_window: None,
        max_tokens: None,
        session_scope: openengine_cluster_protocol::SessionScope::Execution,
        connections: openengine_cluster_protocol::DeclaredConnections::empty(),
    };
    runtime.nodes.insert(
        openengine_cluster_protocol::NodeName::new("worker").unwrap(),
        worker,
    );
    assert!(matches!(
        registry.validate_runtime(&runtime),
        Err(HarnessError::UnknownHarness(_))
    ));
    runtime.nodes.clear();
    runtime.provider = openengine_cluster_protocol::ProviderId::new("unsupported").unwrap();
    assert!(matches!(
        registry.validate_runtime(&runtime),
        Err(HarnessError::UnsupportedProvider(_))
    ));
}

#[test]
fn core_and_local_catalogs_apply_only_the_compiled_contribution_policy() {
    let core = HarnessRegistry::core();
    assert!(
        core.descriptors()
            .all(|descriptor| matches!(descriptor.source, HarnessSource::Core))
    );
    let local = HarnessRegistry::local();
    for lane in core.lanes() {
        local.descriptor(&lane).unwrap();
    }
    for (harness, provider) in [
        ("opencode", "openai"),
        ("opencode", "github"),
        ("hermes", "openai"),
        ("hermes", "github"),
        ("hermes", "bedrock"),
    ] {
        let lane = RuntimeLane::new(harness, provider).unwrap();
        assert!(matches!(
            core.descriptor(&lane),
            Err(HarnessError::UnknownHarness(_))
        ));
        #[cfg(feature = "contributed-harnesses")]
        assert!(matches!(
            local.descriptor(&lane).unwrap().source,
            HarnessSource::Contributed
        ));
        #[cfg(not(feature = "contributed-harnesses"))]
        assert!(matches!(
            local.descriptor(&lane),
            Err(HarnessError::UnknownHarness(_))
        ));
    }
}

#[test]
fn configured_harness_factory_rejects_lane_changes_before_adapter_construction() {
    let registry = HarnessRegistry::core();
    let original = RuntimeLane::new("codex", "openrouter").unwrap();
    for changed in [
        RuntimeLane::new("claude", "openrouter").unwrap(),
        RuntimeLane::new("codex", "openai").unwrap(),
    ] {
        let mut configured = registry
            .configure(HarnessBuildContext {
                lane: original.clone(),
                workspace: std::path::PathBuf::from("unopened-workspace"),
                runtime_home: std::path::PathBuf::from("unopened-runtime"),
                executable: std::path::PathBuf::from("never-launched"),
                prefix_arguments: Vec::new(),
                search_path: String::new(),
                base_environment: Default::default(),
                host: HarnessHost::Local {
                    home: None,
                    environment: Default::default(),
                },
            })
            .unwrap();
        configured.context.lane = changed;
        assert_eq!(configured.lane(), original);
        assert!(matches!(
            configured.build(None),
            Err(HarnessError::Configuration(_))
        ));
    }
}
