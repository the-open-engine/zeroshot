//! Public built-in template selection and local materialization seams.

use openengine_cluster_protocol::{GraphSpec, NodeName, SessionScope};
use thiserror::Error;

use crate::native_v2_contract::{DeclaredConnections, NodeRuntimeBinding};

use super::{auto_research_graph, node_name, single_worker_graph, software_change_graph, DELIVERY_NODE};

/// The deliberately small set of built-in graph templates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinGraphTemplate {
    SingleWorker,
    SoftwareChange,
    AutoResearch,
    IssueReview,
    PrReview,
    CodeReview,
    DocumentationTask,
    CrossProviderTask,
    ServiceTask,
}

impl BuiltinGraphTemplate {
    pub(crate) const ALL: [Self; 9] = [
        Self::SingleWorker,
        Self::SoftwareChange,
        Self::AutoResearch,
        Self::IssueReview,
        Self::PrReview,
        Self::CodeReview,
        Self::DocumentationTask,
        Self::CrossProviderTask,
        Self::ServiceTask,
    ];

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
            Self::IssueReview => "issue-review",
            Self::PrReview => "pr-review",
            Self::CodeReview => "code-review",
            Self::DocumentationTask => "documentation-task",
            Self::CrossProviderTask => "cross-provider-task",
            Self::ServiceTask => "service-task",
        }
    }

    #[must_use]
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|template| template.name() == value)
    }

    pub(crate) const fn default_delivery(self) -> TemplateDelivery {
        match self {
            Self::SoftwareChange => TemplateDelivery::PullRequest,
            Self::IssueReview | Self::PrReview | Self::CodeReview => TemplateDelivery::Comment,
            _ => TemplateDelivery::None,
        }
    }

    pub(crate) fn default_session_scope(self, node: &str) -> SessionScope {
        match (self, node) {
            (Self::SoftwareChange | Self::CrossProviderTask, "worker")
            | (Self::AutoResearch, "explorer" | "synthesizer" | "challenger" | "researcher") => {
                SessionScope::NodeInstance
            }
            _ => SessionScope::Execution,
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
            Self::IssueReview => super::review::issue_review_graph(delivery),
            Self::PrReview => super::pr_review::pr_review_graph(delivery),
            Self::CodeReview => super::code_review::code_review_graph(delivery),
            Self::DocumentationTask => super::focused_worker_graph(
                "Complete the requested documentation task. Check the implementation and existing \
                 documentation, make focused documentation changes, and verify examples and links.",
            ),
            Self::CrossProviderTask => software_change_graph(delivery),
            Self::ServiceTask => super::focused_worker_graph(
                "Complete the requested service integration task. PROJECT_SERVICE_ENDPOINT and \
                 PROJECT_SERVICE_API_KEY are supplied through a named connection. Follow repository \
                 guidance, keep credentials out of files and output, and verify the requested behavior.",
            ),
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
        let connections = DeclaredConnections::empty();
        let name = match self {
            Self::AutoResearch => "checkpoint_delivery",
            Self::SingleWorker
            | Self::DocumentationTask
            | Self::CrossProviderTask
            | Self::ServiceTask
            | Self::SoftwareChange
            | Self::IssueReview
            | Self::PrReview
            | Self::CodeReview => DELIVERY_NODE,
        };
        Ok(Some((
            node_name(name)?,
            if delivery == TemplateDelivery::Comment {
                NodeRuntimeBinding::CommentDelivery { connections }
            } else {
                NodeRuntimeBinding::GitDelivery {
                    connections,
                    pull_request_feedback,
                }
            },
        )))
    }

    fn validate_delivery(self, delivery: TemplateDelivery) -> Result<(), BuiltinTemplateError> {
        let supported = match self {
            Self::SingleWorker
            | Self::DocumentationTask
            | Self::CrossProviderTask
            | Self::ServiceTask => delivery == TemplateDelivery::None,
            Self::SoftwareChange => delivery != TemplateDelivery::Comment,
            Self::AutoResearch => {
                matches!(delivery, TemplateDelivery::None | TemplateDelivery::Push)
            }
            Self::IssueReview => {
                matches!(delivery, TemplateDelivery::None | TemplateDelivery::Comment)
            }
            Self::PrReview | Self::CodeReview => delivery == TemplateDelivery::Comment,
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
    Comment,
}

impl TemplateDelivery {
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Push => "push",
            Self::PullRequest => "pull_request",
            Self::Merge => "merge",
            Self::Comment => "comment",
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
