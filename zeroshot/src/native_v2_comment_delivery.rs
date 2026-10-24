//! Trusted, write-once comment delivery. Publication facts never roll back with the workspace.
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use openengine_cluster_protocol::{EnumLabel, EnvironmentVariableName, FieldName, RunId, WorkerOutcome};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::native_v2_contract::{NodeInvocation, NodeRuntimeBinding};
use crate::native_v2_delivery::TrustedGitHubTokenFile;
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NodeDriver, NodeRunnerError, NodeSession, ResolvedEnvironment,
    SessionFactory,
};
use crate::source_code_provider::{
    SourceComment, SourceCommentAuthority, SourceCommentDestination, SourceDeliveryCredential,
    SourceDeliveryError, SourceDiscussionKind,
};

pub mod contract;
mod store;
pub use contract::*;
pub use store::*;

const MAX_BODY_BYTES: usize = 60 * 1024;
const MAX_COMMENT_PAGES: u32 = 100;
const MARKER_PREFIX: &str = "<!-- zeroshot-comment:v1:";

#[derive(Clone, Copy)]
struct Publication<'a> {
    destination: &'a SourceCommentDestination,
    record: &'a PublicationRecord,
    body: &'a str,
}

struct Confirmation<'a> {
    previous: Option<&'a PublicationRecord>,
    intended: PublicationRecord,
    comment: SourceComment,
    destination: &'a SourceCommentDestination,
}

struct PreparedPublication {
    destination: SourceCommentDestination,
    record: PublicationRecord,
    body: String,
    scope: Option<Value>,
}

impl PreparedPublication {
    fn borrowed(&self) -> Publication<'_> {
        Publication {
            destination: &self.destination,
            record: &self.record,
            body: &self.body,
        }
    }
}

fn validate_previous(
    previous: &PublicationRecord,
    intended: &PublicationRecord,
) -> Result<(), NodeRunnerError> {
    previous.validate().map_err(store_failure)?;
    if previous.url != intended.url
        || previous.marker != intended.marker
        || previous.author != intended.author
    {
        return Err(failure(
            "comment publication identity differs from the retained lineage",
        ));
    }
    if previous.body_hash != intended.body_hash {
        let suffix = match &previous.status {
            PublicationStatus::Confirmed { comment_url, .. } => format!(": {comment_url}"),
            _ => String::new(),
        };
        return Err(failure(format!(
            "comment_already_delivered: finalized body differs{suffix}"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct NativeV2CommentDeliveryConfig {
    pub delivery_run_id: RunId,
    pub expected_url: String,
    pub expected_review_scope: Option<Value>,
    pub source_revision: Option<String>,
}

#[derive(Clone)]
pub struct NativeV2CommentDeliveryAdapter {
    config: Arc<NativeV2CommentDeliveryConfig>,
    authority: Arc<dyn SourceCommentAuthority>,
    store: Arc<dyn CommentPublicationStore>,
    token: Option<Arc<str>>,
    token_file: Option<TrustedGitHubTokenFile>,
}

impl NativeV2CommentDeliveryAdapter {
    #[must_use]
    pub fn new(
        config: NativeV2CommentDeliveryConfig,
        authority: Arc<dyn SourceCommentAuthority>,
        store: Arc<dyn CommentPublicationStore>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            authority,
            store,
            token: None,
            token_file: None,
        }
    }

    #[must_use]
    pub fn with_trusted_github_token(mut self, token: Option<Arc<str>>) -> Self {
        self.token = token;
        self
    }

    #[must_use]
    pub fn with_trusted_github_token_file(
        mut self,
        token_file: Option<TrustedGitHubTokenFile>,
    ) -> Self {
        self.token_file = token_file;
        self
    }

    fn prepare_input(
        &self,
        invocation: &DriverInvocation,
    ) -> Result<contract::CommentInput, NodeRunnerError> {
        validate_comment_contract(&invocation.response)?;
        let input: contract::CommentInput = serde_json::from_value(invocation.node.input.clone())
            .map_err(|_| failure("invalid comment input"))?;
        if input.url != self.config.expected_url
            || input.review_scope != self.config.expected_review_scope
        {
            return Err(failure(
                "comment destination or review scope changed after admission",
            ));
        }
        if input.body.trim().is_empty()
            || input.body.len() > MAX_BODY_BYTES
            || input.body.contains("<!-- zeroshot-")
        {
            return Err(failure(
                "comment body is empty, oversized, or contains a reserved marker",
            ));
        }
        Ok(input)
    }

    async fn publish(
        &self,
        invocation: &DriverInvocation,
        control: &DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let input = self.prepare_input(invocation)?;
        let mut credentials = Credentials::new(self, &invocation.environment)?;
        let author = self.authenticated_actor(&mut credentials, control).await?;
        let publication = self.prepare_publication(invocation, input, author)?;
        let previous = self.store.load().await.map_err(store_failure)?;
        if let Some(outcome) = self
            .reuse_confirmed(&publication, previous.as_ref(), (&mut credentials, control))
            .await?
        {
            return Ok(outcome);
        }
        self.settle_publication(publication, previous.as_ref(), (&mut credentials, control))
            .await
    }

    async fn authenticated_actor(
        &self,
        credentials: &mut Credentials<'_>,
        control: &DriverControl,
    ) -> Result<String, NodeRunnerError> {
        active(control)?;
        let result = match self.authority.actor(credentials.current()).await {
            Err(error) if error.authentication_failed() && credentials.refresh().await? => {
                self.authority.actor(credentials.current()).await
            }
            result => result,
        };
        result.map_err(|error| credentials.failure(error))
    }

    async fn reuse_confirmed(
        &self,
        publication: &PreparedPublication,
        previous: Option<&PublicationRecord>,
        context: (&mut Credentials<'_>, &DriverControl),
    ) -> Result<Option<WorkerOutcome>, NodeRunnerError> {
        let (credentials, control) = context;
        let Some(previous) = previous else {
            return Ok(None);
        };
        validate_previous(previous, &publication.record)?;
        let PublicationStatus::Confirmed { comment_id, .. } = &previous.status else {
            return Ok(None);
        };
        active(control)?;
        let comment = self
            .read_comment(publication.borrowed(), comment_id, credentials)
            .await?;
        validate_comment(
            &comment,
            &publication.destination,
            &publication.record,
            &publication.body,
        )?;
        receipt(
            &comment,
            &publication.destination,
            &publication.record,
            self.config.source_revision.as_deref(),
        )
        .map(Some)
    }

    fn prepare_publication(
        &self,
        invocation: &DriverInvocation,
        input: contract::CommentInput,
        author: String,
    ) -> Result<PreparedPublication, NodeRunnerError> {
        if !valid_id(&author) {
            return Err(failure("invalid comment author identity"));
        }
        let destination =
            crate::source_code_provider::adapters::core::comment_destination(&input.url)
                .map_err(|error| failure(error.to_string()))?;
        let marker = marker(
            &self.config.delivery_run_id,
            invocation.node.reference.node.as_str(),
        );
        let body = finalized_body(&input, &marker, self.config.source_revision.as_deref());
        if body.len() > MAX_BODY_BYTES {
            return Err(failure("final comment body exceeds its size limit"));
        }
        let record = PublicationRecord {
            version: 1,
            url: input.url,
            marker,
            body_hash: hash(&body),
            author,
            status: PublicationStatus::Attempted,
        };
        record.validate().map_err(store_failure)?;
        Ok(PreparedPublication {
            destination,
            record,
            body,
            scope: input.review_scope,
        })
    }

    async fn settle_publication(
        &self,
        publication: PreparedPublication,
        previous: Option<&PublicationRecord>,
        context: (&mut Credentials<'_>, &DriverControl),
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let (credentials, control) = context;
        if let Some(comment) = self
            .reconcile(publication.borrowed(), credentials, control)
            .await?
        {
            return self
                .confirm(Confirmation {
                    previous,
                    intended: publication.record,
                    comment,
                    destination: &publication.destination,
                })
                .await;
        }
        if previous.is_some_and(|record| record.status == PublicationStatus::Attempted) {
            return Err(failure(
                "delivery_unconfirmed: a prior create may have published; no further POST is authorized",
            ));
        }
        self.authorize_create(&publication, previous, (credentials, control))
            .await?;
        let comment = self
            .create_or_reconcile(&publication, credentials, control)
            .await?;
        let result = self
            .confirm(Confirmation {
                previous: Some(&publication.record),
                intended: publication.record.clone(),
                comment,
                destination: &publication.destination,
            })
            .await;
        active(control)?;
        result
    }

    async fn authorize_create(
        &self,
        publication: &PreparedPublication,
        previous: Option<&PublicationRecord>,
        context: (&mut Credentials<'_>, &DriverControl),
    ) -> Result<(), NodeRunnerError> {
        let (credentials, control) = context;
        active(control)?;
        self.read_scope(publication, credentials).await?;
        active(control)?;
        self.store
            .compare_and_set(previous, &publication.record)
            .await
            .map_err(store_failure)?;
        active(control)
    }

    async fn create_or_reconcile(
        &self,
        publication: &PreparedPublication,
        credentials: &mut Credentials<'_>,
        control: &DriverControl,
    ) -> Result<SourceComment, NodeRunnerError> {
        loop {
            match self
                .authority
                .create_comment(
                    &publication.destination,
                    &publication.body,
                    credentials.current(),
                )
                .await
            {
                Ok(comment) => {
                    validate_comment(
                        &comment,
                        &publication.destination,
                        &publication.record,
                        &publication.body,
                    )?;
                    return Ok(comment);
                }
                Err(error) if definite_nonpublication(&error) => {
                    self.record_nonpublication(&publication.record).await?;
                    if self
                        .retry_authentication(publication, &error, (credentials, control))
                        .await?
                    {
                        continue;
                    }
                    return Err(credentials.failure(error));
                }
                Err(error) => {
                    return self
                        .reconcile_uncertain(publication, &error, (credentials, control))
                        .await;
                }
            }
        }
    }

    async fn record_nonpublication(
        &self,
        record: &PublicationRecord,
    ) -> Result<(), NodeRunnerError> {
        let mut ready = record.clone();
        ready.status = PublicationStatus::Ready;
        self.store
            .compare_and_set(Some(record), &ready)
            .await
            .map_err(store_failure)
    }

    async fn retry_authentication(
        &self,
        publication: &PreparedPublication,
        error: &SourceDeliveryError,
        context: (&mut Credentials<'_>, &DriverControl),
    ) -> Result<bool, NodeRunnerError> {
        let (credentials, control) = context;
        if !error.authentication_failed() || !credentials.refresh().await? {
            return Ok(false);
        }
        self.verify_actor(credentials, &publication.record.author)
            .await?;
        let mut ready = publication.record.clone();
        ready.status = PublicationStatus::Ready;
        active(control)?;
        self.store
            .compare_and_set(Some(&ready), &publication.record)
            .await
            .map_err(store_failure)?;
        active(control)?;
        Ok(true)
    }

    async fn refresh_read_auth(
        &self,
        error: &SourceDeliveryError,
        authorization: (&mut Credentials<'_>, &str),
    ) -> Result<bool, NodeRunnerError> {
        let (credentials, author) = authorization;
        if !error.authentication_failed() || !credentials.refresh().await? {
            return Ok(false);
        }
        self.verify_actor(credentials, author).await?;
        Ok(true)
    }

    async fn read_comment(
        &self,
        publication: Publication<'_>,
        id: &str,
        credentials: &mut Credentials<'_>,
    ) -> Result<SourceComment, NodeRunnerError> {
        let result = match self
            .authority
            .get_comment(publication.destination, id, credentials.current())
            .await
        {
            Err(error)
                if self
                    .refresh_read_auth(&error, (credentials, &publication.record.author))
                    .await? =>
            {
                self.authority
                    .get_comment(publication.destination, id, credentials.current())
                    .await
            }
            result => result,
        };
        result.map_err(|error| credentials.failure(error))
    }

    async fn read_page(
        &self,
        publication: Publication<'_>,
        page: u32,
        credentials: &mut Credentials<'_>,
    ) -> Result<crate::source_code_provider::SourceCommentPage, NodeRunnerError> {
        let result = match self
            .authority
            .list_comments(publication.destination, page, credentials.current())
            .await
        {
            Err(error)
                if self
                    .refresh_read_auth(&error, (credentials, &publication.record.author))
                    .await? =>
            {
                self.authority
                    .list_comments(publication.destination, page, credentials.current())
                    .await
            }
            result => result,
        };
        result.map_err(|error| credentials.failure(error))
    }

    async fn read_scope(
        &self,
        publication: &PreparedPublication,
        credentials: &mut Credentials<'_>,
    ) -> Result<(), NodeRunnerError> {
        let Some(scope) = &publication.scope else {
            return Ok(());
        };
        let result = match self
            .authority
            .verify_review_scope(&publication.destination, scope, credentials.current())
            .await
        {
            Err(error)
                if self
                    .refresh_read_auth(&error, (credentials, &publication.record.author))
                    .await? =>
            {
                self.authority
                    .verify_review_scope(&publication.destination, scope, credentials.current())
                    .await
            }
            result => result,
        };
        result.map_err(|error| credentials.failure(error))
    }

    async fn verify_actor(
        &self,
        credentials: &Credentials<'_>,
        expected: &str,
    ) -> Result<(), NodeRunnerError> {
        let observed = self
            .authority
            .actor(credentials.current())
            .await
            .map_err(|error| credentials.failure(error))?;
        if observed != expected {
            return Err(failure(
                "refreshed comment credential changed the publication author",
            ));
        }
        Ok(())
    }

    async fn reconcile_uncertain(
        &self,
        publication: &PreparedPublication,
        error: &SourceDeliveryError,
        context: (&mut Credentials<'_>, &DriverControl),
    ) -> Result<SourceComment, NodeRunnerError> {
        let (credentials, control) = context;
        if error.cleanup_unconfirmed() {
            return Err(NodeRunnerError::CleanupUnconfirmed);
        }
        active(control)?;
        self.reconcile(publication.borrowed(), credentials, control).await?
            .ok_or_else(|| failure("delivery_unconfirmed: the create result is uncertain; retained intent forbids another POST"))
    }

    async fn confirm(
        &self,
        confirmation: Confirmation<'_>,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let Confirmation {
            previous,
            mut intended,
            comment,
            destination,
        } = confirmation;
        intended.status = PublicationStatus::Confirmed {
            comment_id: comment.id.clone(),
            comment_url: comment.url.clone(),
        };
        self.store
            .compare_and_set(previous, &intended)
            .await
            .map_err(store_failure)?;
        receipt(
            &comment,
            destination,
            &intended,
            self.config.source_revision.as_deref(),
        )
    }

    async fn reconcile(
        &self,
        publication: Publication<'_>,
        credentials: &mut Credentials<'_>,
        control: &DriverControl,
    ) -> Result<Option<SourceComment>, NodeRunnerError> {
        let Publication {
            destination,
            record: intended,
            body,
        } = publication;
        let mut found = None;
        for page in 1..=MAX_COMMENT_PAGES {
            active(control)?;
            let result = self.read_page(publication, page, credentials).await?;
            for comment in result.comments {
                if comment.author == intended.author
                    && comment.body.lines().any(|line| line == intended.marker)
                {
                    validate_comment(&comment, destination, intended, body)?;
                    if found.replace(comment).is_some() {
                        return Err(failure("multiple comments match this publication identity"));
                    }
                }
            }
            if !result.has_more {
                return Ok(found);
            }
        }
        Err(failure(
            "delivery_unconfirmed: comment scan exceeded its page bound",
        ))
    }
}

fn definite_nonpublication(error: &SourceDeliveryError) -> bool {
    error
        .api_status()
        .is_some_and(|status| matches!(status, 400 | 401 | 403 | 404 | 405 | 410 | 422))
        || error.authentication_failed()
}

struct Credentials<'a> {
    adapter: &'a NativeV2CommentDeliveryAdapter,
    environment: &'a ResolvedEnvironment,
    token: String,
    declared: bool,
    refreshed: bool,
}

impl<'a> Credentials<'a> {
    fn new(
        adapter: &'a NativeV2CommentDeliveryAdapter,
        environment: &'a ResolvedEnvironment,
    ) -> Result<Self, NodeRunnerError> {
        let name = EnvironmentVariableName::new(adapter.authority.comment_credential_field())
            .map_err(|_| NodeRunnerError::Driver)?;
        let declared = environment.get(&name);
        let token = if let Some(token) = declared {
            if token.is_empty()
                || token.len() > 4096
                || !token.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return Err(failure("declared comment credential is invalid"));
            }
            token.to_owned()
        } else if let Some(file) = &adapter.token_file {
            file.read()
                .map_err(|_| failure("trusted comment credential file is unavailable"))?
        } else {
            adapter.token.as_deref().unwrap_or_default().to_owned()
        };
        Ok(Self {
            adapter,
            environment,
            token,
            declared: declared.is_some(),
            refreshed: false,
        })
    }
    fn current(&self) -> SourceDeliveryCredential<'_> {
        SourceDeliveryCredential::token(&self.token)
    }
    async fn refresh(&mut self) -> Result<bool, NodeRunnerError> {
        if self.refreshed {
            return Ok(false);
        }
        self.refreshed = true;
        if !self.declared {
            if let Some(file) = &self.adapter.token_file {
                self.token = file
                    .read_rotated(&self.token)
                    .map_err(|_| failure("comment credential refresh unavailable"))?;
                return Ok(true);
            }
        }
        if !self.environment.can_refresh() {
            return Ok(false);
        }
        let environment = crate::native_v2_runner::refresh_environment(self.environment)
            .await
            .map_err(|_| failure("comment credential refresh unavailable"))?;
        let name = EnvironmentVariableName::new(self.adapter.authority.comment_credential_field())
            .map_err(|_| NodeRunnerError::Driver)?;
        self.token = environment
            .get(&name)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 4096
                    && value.bytes().all(|byte| byte.is_ascii_graphic())
            })
            .ok_or_else(|| failure("comment credential refresh returned no valid authority"))?
            .to_owned();
        Ok(true)
    }
    fn failure(&self, error: SourceDeliveryError) -> NodeRunnerError {
        if error.cleanup_unconfirmed() {
            return NodeRunnerError::CleanupUnconfirmed;
        }
        let mut detail = error.to_string();
        for secret in self
            .environment
            .iter()
            .map(|(_, value)| value)
            .chain([self.token.as_str()])
        {
            if !secret.is_empty() {
                detail = detail.replace(secret, "[REDACTED]");
            }
        }
        failure(detail)
    }
}

fn failure(message: impl Into<String>) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.into())
}
fn store_failure(error: PublicationStoreError) -> NodeRunnerError {
    failure(error.to_string())
}
fn active(control: &DriverControl) -> Result<(), NodeRunnerError> {
    if control.is_cancelled() {
        Err(NodeRunnerError::Cancelled)
    } else {
        Ok(())
    }
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn marker(root: &RunId, node: &str) -> String {
    format!(
        "{MARKER_PREFIX}{} -->",
        hash(&format!("zeroshot/comment/v1\0{}\0{node}", root.as_str()))
    )
}
pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

pub(crate) fn valid_comment_permalink(
    destination: &SourceCommentDestination,
    id: &str,
    value: &str,
) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && id != "0"
        && ["issues", "pull"].iter().any(|segment| {
            let suffix = format!("/{segment}/{}#issuecomment-{id}", destination.discussion_id);
            value
                .strip_prefix("https://github.com/")
                .and_then(|value| value.strip_suffix(suffix.as_str()))
                .is_some_and(|repository| repository.eq_ignore_ascii_case(&destination.repository))
        })
}

fn finalized_body(input: &contract::CommentInput, marker: &str, revision: Option<&str>) -> String {
    let mut body = format!("Zeroshot report for {}", input.url);
    let comparison = input.review_scope.as_ref().and_then(|scope| {
        Some((
            scope.get("baseRevision").and_then(Value::as_str)?,
            scope.get("headRevision").and_then(Value::as_str)?,
        ))
    });
    if let Some(revision) = revision
        && !comparison.is_some_and(|(_, head)| head == revision)
    {
        body.push_str(&format!("\n\nSource revision: `{revision}`"));
    }
    if let Some((base, head)) = comparison {
        body.push_str(&format!(
            "\n\nReviewed PR head `{head}` against base `{base}`."
        ));
    }
    format!("{body}\n\n{}\n\n{marker}", input.body)
}

fn validate_comment(
    comment: &SourceComment,
    destination: &SourceCommentDestination,
    record: &PublicationRecord,
    body: &str,
) -> Result<(), NodeRunnerError> {
    if comment.discussion_url != destination.url
        || comment.author != record.author
        || comment.body != body
        || hash(&comment.body) != record.body_hash
        || !valid_comment_permalink(destination, &comment.id, &comment.url)
        || comment
            .body
            .lines()
            .filter(|line| *line == record.marker)
            .count()
            != 1
    {
        return Err(failure(
            "comment identity, author, or finalized content did not match",
        ));
    }
    Ok(())
}

fn receipt(
    comment: &SourceComment,
    destination: &SourceCommentDestination,
    record: &PublicationRecord,
    revision: Option<&str>,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let mut output = json!({"version":"v1","mode":"comment","outcome":"posted","destinationUrl":record.url,"repository":destination.repository,"discussionKind":match destination.kind { SourceDiscussionKind::Issue => "issue", SourceDiscussionKind::PullRequest => "pull_request" },"discussionId":destination.discussion_id,"commentId":comment.id,"url":comment.url,"bodyHash":record.body_hash});
    if let Some(revision) = revision {
        output["sourceRevision"] = json!(revision);
    }
    Ok(WorkerOutcome::Verifier {
        output: json!({"receipt":output}),
        signals: BTreeMap::from([(
            FieldName::new("delivery").map_err(|_| NodeRunnerError::Driver)?,
            EnumLabel::new("posted").map_err(|_| NodeRunnerError::Driver)?,
        )]),
        diagnostic: json!({"message":"Forge confirmed the exact comment"}),
        artifacts: Vec::new(),
    })
}

struct CommentSession(AtomicBool);
#[async_trait]
impl NodeSession for CommentSession {
    fn as_any(&self) -> &dyn Any {
        self
    }
    async fn is_live(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    async fn close(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
#[async_trait]
impl SessionFactory for NativeV2CommentDeliveryAdapter {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        if !matches!(
            invocation.binding,
            NodeRuntimeBinding::CommentDelivery { .. }
        ) || invocation.worker.as_str() != COMMENT_DELIVERY_WORKER_REF
        {
            return Err(NodeRunnerError::InvalidRole);
        }
        Ok(Arc::new(CommentSession(AtomicBool::new(true))))
    }
}
#[async_trait]
impl NodeDriver for NativeV2CommentDeliveryAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if invocation.role != crate::native_v2_runner::NodeRole::CommentDelivery
            || !invocation.session.is_live().await
        {
            return Err(NodeRunnerError::InvalidRole);
        }
        self.publish(&invocation, &control).await
    }
}

#[cfg(test)]
mod tests;
