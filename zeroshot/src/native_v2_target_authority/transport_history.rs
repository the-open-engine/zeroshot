//! Private capability boundary for the native, credential-free history projection.
use openengine_cluster_protocol::{is_canonical_uuid_v7, Cursor, RunId};
use serde::Deserialize;

use super::NativeV2TargetServer;
use super::http::{HttpRequest, HttpResponse, invalid_request_response, not_found_response};
use crate::native_v2_observability::history::{HistoryError, RunHistoryService};

pub(super) const DEFINITION_PATH: &str = "/native-v2/history/definition";
pub(super) const PAGE_PATH: &str = "/native-v2/history/page";
const MAX_HISTORY_REQUEST_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefinitionRequest {
    run_id: RunId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageRequest {
    run_id: RunId,
    after: Option<Cursor>,
}

enum HistoryRequest {
    Definition(DefinitionRequest),
    Page(PageRequest),
}

impl HistoryRequest {
    fn parse(request: &HttpRequest) -> Result<Self, HttpResponse> {
        match request.path.as_str() {
            DEFINITION_PATH => match serde_json::from_slice::<DefinitionRequest>(&request.body) {
                Ok(value) if is_canonical_uuid_v7(&value.run_id) => Ok(Self::Definition(value)),
                _ => Err(invalid_request_response(
                    "history definition request is malformed",
                )),
            },
            PAGE_PATH => match serde_json::from_slice::<PageRequest>(&request.body) {
                Ok(value) if is_canonical_uuid_v7(&value.run_id) => Ok(Self::Page(value)),
                _ => Err(invalid_request_response(
                    "history page request is malformed",
                )),
            },
            _ => Err(not_found_response()),
        }
    }

    async fn respond(self, history: &RunHistoryService) -> HttpResponse {
        match self {
            Self::Definition(request) => match history.definition(&request.run_id).await {
                Ok(value) => HttpResponse::private_json(200, &value),
                Err(error) => history_error(error),
            },
            Self::Page(request) => match history.page(&request.run_id, request.after).await {
                Ok(value) => HttpResponse::private_json(200, &value),
                Err(error) => history_error(error),
            },
        }
    }
}

impl NativeV2TargetServer {
    pub(super) async fn handle_history(&self, request: HttpRequest) -> HttpResponse {
        // Hosted target-wide authentication is deliberately insufficient for this per-run export.
        if let Err(response) = self.authenticate_private_control(&request.head).await {
            return response;
        }
        if request.body.len() > MAX_HISTORY_REQUEST_BYTES {
            return invalid_request_response("history request exceeds its byte limit");
        }
        let parsed = match HistoryRequest::parse(&request) {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
        let Some(history) = self.target.history().await else {
            return history_error(crate::native_v2_observability::history::unavailable());
        };
        parsed.respond(&history).await
    }
}

fn history_error(error: HistoryError) -> HttpResponse {
    HttpResponse::problem(error.status, error.code, &error.message)
}
