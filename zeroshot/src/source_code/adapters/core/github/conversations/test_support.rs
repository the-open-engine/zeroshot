use std::path::{Path, PathBuf};
use serde_json::{Value, json};
use openengine_cluster_testkit::fixture::{python3_script, write_executable};
use crate::source_code_provider::{SourceReviewFeedback, SourceReviewReceipt, SourceReviewResponse};

pub(crate) fn fixture(root: &Path, receipt: &SourceReviewReceipt, kind: &str) -> PathBuf {
    let program = root.join("gh-conversations");
    write_executable(&program, python3_script(include_str!("fixture.py")), 0o700).unwrap();
    std::fs::write(format!("{}.review", program.display()), json!({
        "number":17,"base":{"ref":receipt.target_branch,"sha":"a".repeat(40),"repo":{"full_name":receipt.repository}},
        "head":{"ref":receipt.head_branch,"sha":receipt.head_revision,"repo":{"full_name":receipt.repository}},
    }).to_string()).unwrap();
    save(
        &program,
        &json!({"threads":[{"id":"THREAD_1","resolved":false,"comments":[{
        "id":41,"updated_at":"one","user":{"id":7,"login":"reviewer","type":kind},
        "body":"Handle empty input.","path":"src/lib.rs","original_line":7,"commit_id":receipt.head_revision,"in_reply_to_id":null
    }]}],"permission":"read"}),
    );
    program
}

pub(crate) fn state(program: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{}.state", program.display())).unwrap())
        .unwrap()
}

pub(crate) fn save(program: &Path, state: &Value) {
    std::fs::write(format!("{}.state", program.display()), state.to_string()).unwrap();
}

pub(crate) fn response(feedback: &SourceReviewFeedback) -> SourceReviewResponse {
    SourceReviewResponse {
        thread_id: feedback.conversations[0].id.clone(),
        version: feedback.conversations[0].version.clone(),
        disposition: crate::source_code_provider::SourceReviewDisposition::Addressed,
        response: "Added the empty-input guard and checked the regression.".into(),
    }
}
