//! Public built-in template selection and local materialization seams.

use openengine_cluster_protocol::{GraphSpec, NodeName};
use thiserror::Error;

use crate::native_v2_contract::{
    ConnectionKey, DeclaredConnections, DeclaredEnvironment, EnvironmentVariableName,
    GITHUB_CONNECTION_KEY, NodeRuntimeBinding,
};
use crate::native_v2_delivery::GITHUB_TOKEN_ENV;

use super::{
    auto_research_graph, node_name, single_worker_graph, software_change_graph, static_value,
    DELIVERY_NODE,
};

/// The deliberately small set of built-in graph templates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinGraphTemplate {
    SingleWorker,
    SoftwareChange,
    AutoResearch,
}

impl BuiltinGraphTemplate {
    pub(crate) const ALL: [Self; 3] =
        [Self::SingleWorker, Self::SoftwareChange, Self::AutoResearch];

    #[must_use]
    pub(crate) const fn all() -> &'static [Self] {
        &Self::ALL
    }

    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::SingleWorker => "single-worker",
            Self::SoftwareChange => "software-change",
            Self::AutoResearch => "auto-research",
        }
    }

    #[must_use]
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "single-worker" => Some(Self::SingleWorker),
            "software-change" => Some(Self::SoftwareChange),
            "auto-research" => Some(Self::AutoResearch),
            _ => None,
        }
    }

    pub(crate) fn materialize(
        self,
        delivery: TemplateDelivery,
    ) -> Result<GraphSpec, BuiltinTemplateError> {
        self.validate_delivery(delivery)?;
        match self {
            Self::SingleWorker => single_worker_graph(),
            Self::SoftwareChange => software_change_graph(delivery),
            Self::AutoResearch => auto_research_graph(delivery),
        }
    }

    #[cfg(any(test, feature = "workspace"))]
    pub(crate) fn delivery_runtime_binding(
        self,
        delivery: TemplateDelivery,
    ) -> Result<Option<(NodeName, NodeRuntimeBinding)>, BuiltinTemplateError> {
        self.delivery_runtime_binding_with_feedback(
            delivery,
            crate::native_v2_contract::PullRequestFeedback::Consider,
        )
    }

    pub(crate) fn delivery_runtime_binding_with_feedback(
        self,
        delivery: TemplateDelivery,
        pull_request_feedback: crate::native_v2_contract::PullRequestFeedback,
    ) -> Result<Option<(NodeName, NodeRuntimeBinding)>, BuiltinTemplateError> {
        self.validate_delivery(delivery)?;
        if delivery == TemplateDelivery::None {
            return Ok(None);
        }
        let environment_name = static_value(EnvironmentVariableName::new(GITHUB_TOKEN_ENV))?;
        let environment = static_value(DeclaredEnvironment::new([environment_name]))?;
        let connection_key = static_value(ConnectionKey::new(GITHUB_CONNECTION_KEY))?;
        let connections = static_value(DeclaredConnections::new([(connection_key, environment)]))?;
        let name = match self {
            Self::AutoResearch => "checkpoint_delivery",
            Self::SingleWorker | Self::SoftwareChange => DELIVERY_NODE,
        };
        Ok(Some((
            node_name(name)?,
            NodeRuntimeBinding::GitDelivery {
                connections,
                pull_request_feedback,
            },
        )))
    }

    fn validate_delivery(self, delivery: TemplateDelivery) -> Result<(), BuiltinTemplateError> {
        let supported = match self {
            Self::SingleWorker => delivery == TemplateDelivery::None,
            Self::SoftwareChange => true,
            Self::AutoResearch => {
                matches!(delivery, TemplateDelivery::None | TemplateDelivery::Push)
            }
        };
        if supported {
            Ok(())
        } else {
            Err(BuiltinTemplateError::UnsupportedDelivery {
                template: self.name(),
                delivery,
            })
        }
    }
}

/// Closed delivery materialization for built-in templates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateDelivery {
    None,
    Push,
    PullRequest,
    Merge,
}

impl TemplateDelivery {
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Push => "push",
            Self::PullRequest => "pull_request",
            Self::Merge => "merge",
        }
    }
}

impl std::fmt::Display for TemplateDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub(crate) enum BuiltinTemplateError {
    #[error("template {template} does not support delivery mode {delivery}")]
    UnsupportedDelivery {
        template: &'static str,
        delivery: TemplateDelivery,
    },
    #[error("a built-in graph template contains an invalid static contract")]
    InvalidStaticContract,
}
