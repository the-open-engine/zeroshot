use super::*;
use crate::native_v2_hosting::environment::HookRequest;

pub(super) struct HookContext<'a> {
    pub state: &'a ProductionCapsuleState,
    pub process_pool: HostedProcessPool,
    pub phase: HookPhase,
}

impl ProductionCapsuleAllocator {
    pub(super) async fn run_environment_hook(
        &self,
        request: &CapsuleBuildRequest<'_>,
        filesystem: &CapsuleFilesystem,
        context: HookContext<'_>,
    ) -> Result<(), CapsuleAllocationUnavailable> {
        let Some(definition) = request.admitted.environment.as_ref() else {
            return Ok(());
        };
        let HookContext {
            state,
            process_pool: pool,
            phase,
        } = context;
        let identity = pool
            .identity(HostedProcessScope::Environment)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let home = environment::prepare_environment(
            &state.run_root,
            &filesystem.runtime_home,
            identity,
            phase,
        )?;
        let mut values = request
            .preparation
            .environment
            .preparation_values(definition)
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let redactions: Vec<String> = definition
            .connections
            .environment_names()
            .filter_map(|name| values.get(name.as_str()))
            .flat_map(|value| value.lines().map(str::to_owned))
            .collect();
        values.insert(
            "ZEROSHOT_TOOLS".to_owned(),
            environment::tools_directory(&state.run_root)
                .to_string_lossy()
                .into_owned(),
        );
        values.insert(
            "PATH".to_owned(),
            environment::search_path(&state.run_root, &self.config.executable_search_path),
        );
        values.insert("HOME".to_owned(), home.to_string_lossy().into_owned());
        values.insert("LANG".to_owned(), "C.UTF-8".to_owned());
        let (script, directory) = match phase {
            HookPhase::Setup => (definition.setup.as_deref(), state.run_root.as_path()),
            HookPhase::Startup => (
                definition.startup.as_deref(),
                filesystem.workspace.as_path(),
            ),
        };
        state
            .environment
            .run(HookRequest {
                phase,
                script,
                directory,
                identity,
                environment: &values,
                preparation: &request.preparation,
                redactions: &redactions,
            })
            .await
    }
}
