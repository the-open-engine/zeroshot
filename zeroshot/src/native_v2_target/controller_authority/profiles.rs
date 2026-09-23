use openengine_cluster_protocol::{
    RunProfile, RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
    RunProfileListRequest, RunProfileListResult, RunProfileMutationResult, RunProfileRunRequest,
    RunProfileSelector, RunProfileSetRequest, RunSubmitResult,
};
use reqwest::header::{ACCEPT, CACHE_CONTROL};

use super::contract::{RunProfilesDescriptor, authority_error};
use super::TargetHttpControlAuthority;
use super::access::AccessToken;
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord};

enum ProfileOperation {
    List,
    Show,
    Set,
    Delete,
    Default,
    Run,
}

impl ProfileOperation {
    fn route<'a>(&self, routes: &'a RunProfilesDescriptor) -> &'a reqwest::Url {
        match self {
            Self::List => &routes.list,
            Self::Show => &routes.show,
            Self::Set => &routes.set,
            Self::Delete => &routes.delete,
            Self::Default => &routes.default,
            Self::Run => &routes.run,
        }
    }

    const fn label(&self) -> &'static str {
        match self {
            Self::List => "profile list",
            Self::Show => "profile show",
            Self::Set => "profile set",
            Self::Delete => "profile delete",
            Self::Default => "profile default",
            Self::Run => "profile run",
        }
    }
}

impl TargetHttpControlAuthority {
    async fn profile_access(
        &self,
        target: &TargetRecord,
    ) -> Result<(RunProfilesDescriptor, AccessToken), TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "direct target does not advertise profile management",
            ));
        }
        let (auth, controller) = self.descriptors(target).await?;
        let routes = auth.run_profiles.clone().ok_or_else(|| {
            authority_error("hosted target does not advertise profile management")
        })?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        Ok((routes, access))
    }

    async fn profile_json<I, O>(
        &self,
        target: &TargetRecord,
        operation: ProfileOperation,
        input: &I,
    ) -> Result<O, TargetAuthorityError>
    where
        I: serde::Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        let (routes, access) = self.profile_access(target).await?;
        let url = operation.route(&routes).clone();
        let builder = self
            .authorized(self.client.post(url.clone()), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(input);
        self.hosted_json((builder, &access), operation.label(), None)
            .await
    }

    pub(super) async fn profile_list(
        &self,
        target: &TargetRecord,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::List, &request)
            .await
    }

    pub(super) async fn profile_show(
        &self,
        target: &TargetRecord,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Show, &selector)
            .await
    }

    pub(super) async fn profile_set(
        &self,
        target: &TargetRecord,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Set, &request)
            .await
    }

    pub(super) async fn profile_delete(
        &self,
        target: &TargetRecord,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Delete, &selector)
            .await
    }

    pub(super) async fn profile_default(
        &self,
        target: &TargetRecord,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Default, &request)
            .await
    }

    pub(super) async fn profile_run(
        &self,
        target: &TargetRecord,
        request: &RunProfileRunRequest,
    ) -> Result<RunSubmitResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Run, request)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use openengine_cluster_protocol::{RunProfileName, RunProfileScope};
    use openengine_cluster_testkit::admission::graph_fixture;
    use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
    use serde_json::json;

    use super::super::credentials::test_support::{MemoryCredentialStore, MemoryDeviceCodeNotifier};
    use super::*;

    fn direct_target() -> TargetRecord {
        TargetRecord {
            id: "11111111-1111-4111-8111-111111111111".to_owned(),
            name: "vm".to_owned(),
            origin: "http://127.0.0.1:8080".to_owned(),
            access: TargetAccess::Direct,
        }
    }

    fn selector() -> RunProfileSelector {
        RunProfileSelector {
            scope: RunProfileScope::User,
            name: RunProfileName::new("contract").assert_value(),
        }
    }

    fn set_request() -> RunProfileSetRequest {
        RunProfileSetRequest {
            name: selector().name,
            scope: RunProfileScope::User,
            graph: graph_fixture("worker", json!({"kind":"null"})),
            runtime: serde_json::from_value(json!({
                "harness":"codex", "provider":"openai", "size":"small",
                "nodes":{"worker":{"kind":"agent","model":"opaque-model"}}
            }))
            .assert_value(),
            set_default: false,
        }
    }

    fn run_request() -> RunProfileRunRequest {
        serde_json::from_value(json!({
            "runId":"profile-contract-run",
            "profile":{"scope":"user","name":"contract"},
            "title":"Profile contract",
            "initialInput":null,
            "source":{
                "repository":"open-engine/zeroshot",
                "branch":"main",
                "revision":"0123456789abcdef0123456789abcdef01234567"
            },
            "submissionKey":"profile-contract",
            "connections":{}
        }))
        .assert_value()
    }

    #[test]
    fn operations_select_their_exact_route_and_diagnostic_label() {
        let base = reqwest::Url::parse("https://target.example").assert_value();
        let routes = RunProfilesDescriptor {
            list: base.join("/profiles/list").assert_value(),
            show: base.join("/profiles/show").assert_value(),
            set: base.join("/profiles/set").assert_value(),
            delete: base.join("/profiles/delete").assert_value(),
            default: base.join("/profiles/default").assert_value(),
            run: base.join("/profiles/run").assert_value(),
        };
        for (operation, path, label) in [
            (ProfileOperation::List, "/profiles/list", "profile list"),
            (ProfileOperation::Show, "/profiles/show", "profile show"),
            (ProfileOperation::Set, "/profiles/set", "profile set"),
            (
                ProfileOperation::Delete,
                "/profiles/delete",
                "profile delete",
            ),
            (
                ProfileOperation::Default,
                "/profiles/default",
                "profile default",
            ),
            (ProfileOperation::Run, "/profiles/run", "profile run"),
        ] {
            assert_eq!(operation.route(&routes).path(), path);
            assert_eq!(operation.label(), label);
        }
    }

    #[tokio::test]
    async fn direct_targets_reject_every_profile_wrapper_before_any_hosted_effect() {
        let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
            "target-direct-profile-refusal",
        );
        let authority = TargetHttpControlAuthority::with_dependencies(
            Arc::new(MemoryCredentialStore::default()),
            Arc::new(MemoryDeviceCodeNotifier::default()),
            root.path("locks"),
        );
        let target = direct_target();
        let expected = "direct target does not advertise profile management";

        assert_eq!(
            authority
                .profile_list(
                    &target,
                    RunProfileListRequest {
                        scope: RunProfileScope::User,
                    },
                )
                .await
                .assert_error()
                .to_string(),
            expected
        );
        assert_eq!(
            authority
                .profile_show(&target, selector())
                .await
                .assert_error()
                .to_string(),
            expected
        );
        assert_eq!(
            authority
                .profile_set(&target, set_request())
                .await
                .assert_error()
                .to_string(),
            expected
        );
        assert_eq!(
            authority
                .profile_delete(&target, selector())
                .await
                .assert_error()
                .to_string(),
            expected
        );
        assert_eq!(
            authority
                .profile_default(
                    &target,
                    RunProfileDefaultRequest {
                        scope: RunProfileScope::User,
                        name: None,
                    },
                )
                .await
                .assert_error()
                .to_string(),
            expected
        );
        assert_eq!(
            authority
                .profile_run(&target, &run_request())
                .await
                .assert_error()
                .to_string(),
            expected
        );
    }
}
