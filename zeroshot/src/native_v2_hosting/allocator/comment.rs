//! Composition of the native comment publisher for one hosted workspace.
use super::*;

impl ProductionCapsuleAllocator {
    pub(super) fn candidate_comment_delivery(
        &self,
        request: &CapsuleBuildRequest<'_>,
        pending: &PendingCapsule<'_>,
        prepared: &PreparedCapsule,
    ) -> Result<Option<NativeV2CandidateCommentDelivery>, CapsuleAllocationUnavailable> {
        use crate::native_v2_comment_delivery::{
            LocalCommentPublicationStore, NativeV2CommentDeliveryConfig,
        };
        use crate::source_code_provider::adapters::core::{GhCliAuthorityConfig, comment_authority};
        let Some(input) = crate::native_v2_admission::admitted_comment_input(request.admitted)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
        else {
            return Ok(None);
        };
        let root = pending
            .state
            .restored_delivery_run_id
            .get()
            .unwrap_or(request.delivery_run_id);
        let mut config = GhCliAuthorityConfig::hosted(prepared.paths.runtime_home.clone());
        config.gh_program = self.config.gh_program.clone();
        let authority = comment_authority(config, &input.url)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let store = match &prepared.hosted_workspace.comment_publications {
            Some(store) => store.clone(),
            None => Arc::new(LocalCommentPublicationStore::new(
                self.config
                    .storage_root
                    .join("comment-publications")
                    .join(format!("{}.json", root.as_str())),
            )),
        };
        Ok(Some(NativeV2CandidateCommentDelivery {
            config: NativeV2CommentDeliveryConfig {
                delivery_run_id: root.clone(),
                expected_url: input.url,
                expected_review_scope: input.review_scope,
                source_revision: request
                    .admitted
                    .source
                    .as_ref()
                    .map(|source| source.revision.as_str().to_owned()),
            },
            authority,
            store,
        }))
    }
}
