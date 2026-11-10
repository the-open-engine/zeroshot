//! Compose trusted delivery authority for the allocated workspace.
use super::*;
use crate::native_v2_hosting::HostedDeliveryValidationRequest;
use crate::native_v2_candidate::{NativeV2CandidateDelivery, TrustedDeliveryCapabilities};
use crate::native_v2_delivery::{DeliveryLineage, NativeV2DeliveryConfig};
use crate::source_code_provider::adapters::core::SourceDeliveryFactoryRequest;

pub(super) struct PreparedDelivery {
    pub(super) delivery: Option<NativeV2CandidateDelivery>,
    pub(super) trusted: TrustedDeliveryCapabilities,
}

impl ProductionCapsuleAllocator {
    pub(super) async fn prepare_delivery(
        &self,
        request: &CapsuleBuildRequest<'_>,
        pending: &PendingCapsule<'_>,
        prepared: &PreparedCapsule,
    ) -> Result<PreparedDelivery, CapsuleAllocationUnavailable> {
        let delivery = self.candidate_delivery(request, pending, prepared)?;
        let validator = if delivery.is_some() {
            self.prepare_delivery_validation(request, pending, prepared)
                .await?
        } else {
            None
        };
        Ok(PreparedDelivery {
            delivery,
            trusted: TrustedDeliveryCapabilities {
                token: request.github_token.map(Arc::<str>::from),
                validator,
                ..Default::default()
            },
        })
    }

    async fn prepare_delivery_validation(
        &self,
        request: &CapsuleBuildRequest<'_>,
        pending: &PendingCapsule<'_>,
        prepared: &PreparedCapsule,
    ) -> Result<
        Option<crate::native_v2_delivery::validation::TrustedDeliveryValidator>,
        CapsuleAllocationUnavailable,
    > {
        let Some(factory) = &self.config.delivery_validation else {
            return Ok(None);
        };
        let validator = factory
            .prepare(HostedDeliveryValidationRequest {
                run_id: request.run_id,
                delivery_run_id: pending
                    .state
                    .restored_delivery_run_id
                    .get()
                    .unwrap_or(request.delivery_run_id),
                admitted: request.admitted,
                workspace: &prepared.filesystem.workspace,
                runtime_home: &prepared.filesystem.runtime_home,
                process_pool: pending.process_pool,
            })
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        if validator
            .as_ref()
            .is_some_and(|validator| validator.workspace() != prepared.filesystem.workspace)
        {
            return Err(CapsuleAllocationUnavailable::Runtime);
        }
        Ok(validator)
    }

    fn candidate_delivery(
        &self,
        request: &CapsuleBuildRequest<'_>,
        pending: &PendingCapsule<'_>,
        prepared: &PreparedCapsule,
    ) -> Result<Option<NativeV2CandidateDelivery>, CapsuleAllocationUnavailable> {
        let delivery_run_id = pending
            .state
            .restored_delivery_run_id
            .get()
            .unwrap_or(request.delivery_run_id);
        let capabilities =
            crate::native_v2_admission::delivery_capabilities(&request.admitted.graph.root);
        if capabilities.is_empty() {
            Ok(None)
        } else {
            let source = request
                .admitted
                .source
                .as_ref()
                .ok_or(CapsuleAllocationUnavailable::Runtime)?;
            let mode = crate::native_v2_admission::delivery_mode(&request.admitted.graph.root)
                .ok_or(CapsuleAllocationUnavailable::Runtime)?;
            let target = crate::native_v2_delivery::source_target(source, mode)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            let config = NativeV2DeliveryConfig {
                commit_identity: request.commit_identity.cloned(),
                commit_co_author: request.commit_co_author.cloned(),
                ..NativeV2DeliveryConfig::for_hosted_workspace(
                    DeliveryLineage::new(
                        delivery_run_id.clone(),
                        request.adopt_existing_delivery || delivery_run_id != pending.run_id,
                    ),
                    prepared.filesystem.workspace.clone(),
                    target,
                    pending.git_identity,
                )
            };
            NativeV2CandidateDelivery::new(
                config,
                SourceDeliveryFactoryRequest {
                    source,
                    workspace: &prepared.filesystem.workspace,
                    git_program: &self.config.git_program,
                    gh_program: &self.config.gh_program,
                    home_directory: &prepared.paths.runtime_home,
                    private_directory: &prepared.paths.runtime_home,
                    git_identity: Some(pending.git_identity),
                    local_environment: None,
                    required_capabilities: &capabilities,
                    operator_diagnostics: Some((
                        request.run_id.clone(),
                        self.operator_diagnostics.clone(),
                    )),
                },
                request.git_credentials.cloned(),
            )
            .map(Some)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)
        }
    }
}
