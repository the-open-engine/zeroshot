//! Maintained adapters register through the same boundary as contributions.
use std::collections::BTreeMap;
use std::path::PathBuf;
use crate::harnesses::HarnessRegistry;

pub use crate::native_v2_codex as codex;
pub use crate::native_v2_claude as claude;
pub use crate::native_v2_copilot as copilot;
pub use crate::native_v2_pi as pi;

pub(crate) fn register(registry: &mut HarnessRegistry) {
    for registration in [
        codex::registration(),
        claude::registration(),
        copilot::registration(),
        pi::registration(),
    ] {
        registry
            .register(registration)
            .expect("product-owned harness registrations are unique");
    }
}

/// Adapts the existing embedder configuration at the maintained-adapter boundary.
pub(crate) struct ContainedLaunch {
    pub(crate) executable: PathBuf,
    pub(crate) arguments: Vec<String>,
    pub(crate) environment: BTreeMap<String, String>,
}

pub(crate) fn contained_launch(
    config: &crate::native_v2_hosting::ProductionHostingConfig,
    id: &str,
) -> Option<ContainedLaunch> {
    let (executable, arguments, environment) = match id {
        "codex" => (config.codex_executable.clone(), Vec::new(), BTreeMap::new()),
        "copilot" => (
            config.copilot_executable.clone(),
            Vec::new(),
            BTreeMap::new(),
        ),
        "claude" => (
            PathBuf::from(&config.claude_executable),
            config.claude_prefix_arguments.clone(),
            config.claude_process_environment.clone_values(),
        ),
        "pi" => (
            PathBuf::from(&config.pi_executable),
            config.pi_prefix_arguments.clone(),
            config.pi_process_environment.clone_values(),
        ),
        _ => return None,
    };
    Some(ContainedLaunch {
        executable,
        arguments,
        environment,
    })
}
