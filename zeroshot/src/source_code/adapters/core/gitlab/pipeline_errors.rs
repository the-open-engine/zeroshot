//! REST's nullable yaml_errors does not include all persisted pipeline configuration errors.
use serde::Deserialize;
use serde_json::{Value, json};
use super::*;
use super::wire::Pipeline;

const QUERY: &str = r#"
query($project: ID!, $pipeline: CiPipelineID!, $after: String) {
  project(fullPath: $project) {
    id
    pipeline(id: $pipeline) {
      id
      sha
      errorMessages(first: 100, after: $after) {
        nodes { content }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
struct ErrorProject {
    id: String,
    pipeline: ErrorPipeline,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorPipeline {
    id: String,
    sha: String,
    error_messages: ErrorConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorConnection {
    nodes: Vec<ErrorMessage>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
struct ErrorMessage {
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

pub(super) async fn read(
    authority: &GitLabDeliveryAuthority,
    project: &Project,
    head: &Pipeline,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<String>, SourceDeliveryError> {
    let pipeline_id = format!("gid://gitlab/Ci::Pipeline/{}", head.id);
    let project_id = format!("gid://gitlab/Project/{}", project.id);
    let mut after = None;
    let mut cursors = BTreeSet::new();
    let mut errors = Vec::new();
    for _ in 0..api::PAGE_LIMIT {
        let response: Value = authority
            .api
            .json(
                ApiRequest::graphql(
                    QUERY,
                    json!({
                        "project":project.path_with_namespace,
                        "pipeline":pipeline_id,
                        "after":after,
                    }),
                ),
                credential,
            )
            .await?;
        if response.get("errors").is_some_and(|value| {
            !value.is_null() && value.as_array().is_none_or(|errors| !errors.is_empty())
        }) {
            return Err(incomplete());
        }
        let project: ErrorProject = serde_json::from_value(
            response
                .pointer("/data/project")
                .cloned()
                .ok_or_else(incomplete)?,
        )
        .map_err(|_| incomplete())?;
        if project.id != project_id
            || project.pipeline.id != pipeline_id
            || project.pipeline.sha != head.sha
        {
            return Err(SourceDeliveryError::identity(
                "GitLab pipeline errors changed project or commit identity",
            ));
        }
        let messages = project.pipeline.error_messages;
        if messages.nodes.len() > 100 {
            return Err(incomplete());
        }
        errors.extend(messages.nodes.into_iter().map(|message| message.content));
        if !messages.page_info.has_next_page {
            return Ok(errors);
        }
        let cursor = messages
            .page_info
            .end_cursor
            .filter(|cursor| !cursor.is_empty())
            .ok_or_else(incomplete)?;
        if !cursors.insert(cursor.clone()) {
            return Err(incomplete());
        }
        after = Some(cursor);
    }
    Err(incomplete())
}

fn incomplete() -> SourceDeliveryError {
    SourceDeliveryError::api(None, "GitLab returned incomplete pipeline error messages")
}
