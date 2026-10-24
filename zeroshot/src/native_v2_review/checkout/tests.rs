use openengine_cluster_protocol::{FieldPath, GraphNode};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};

fn review() -> AdmittedRun {
    let mut admitted = crate::native_v2_runner::test_support::admitted();
    admitted.graph.root = BuiltinGraphTemplate::IssueReview
        .materialize(TemplateDelivery::Comment)
        .assert_value()
        .root;
    admitted.source = Some(
        serde_json::from_value(json!({
            "repository":"acme/project", "branch":"main", "revision":"a".repeat(40)
        }))
        .assert_value(),
    );
    admitted.initial_input = json!({
        "url":"https://github.com/acme/project/pull/17",
        "context":{"title":"Review","body":"Changes","reviewScope":{
            "baseRepository":"acme/project", "baseBranch":"main", "baseRevision":"b".repeat(40),
            "headRepository":"contributor/project", "headBranch":"feature", "headRevision":"a".repeat(40)
        }}
    });
    admitted
}

#[test]
fn unrelated_graph_input_never_authorizes_review_checkout_preparation() {
    let mut admitted = crate::native_v2_runner::test_support::admitted();
    admitted.initial_input = json!({"context":{"reviewScope":{"baseRevision":"caller data"}}});
    admitted.source = None;
    assert!(checkout_references(&admitted).assert_value().is_empty());
}

#[test]
fn review_checkout_uses_the_publishers_bound_caller_paths_and_exact_source() {
    let mut admitted = review();
    let GraphNode::Seq(root) = &mut admitted.graph.root else {
        panic!("review root")
    };
    let GraphNode::Choice(route) = &mut root.children.as_mut_slice()[1] else {
        panic!("review route")
    };
    let Some(GraphNode::Seq(continuation)) = route.otherwise.as_deref_mut() else {
        panic!("review continuation")
    };
    let GraphNode::Verifier(publisher) = &mut continuation.children.as_mut_slice()[0] else {
        panic!("publisher")
    };
    for binding in &mut publisher.input_bindings {
        let path = match binding.target.segments()[0].as_str() {
            "url" => "destination",
            "reviewScope" => "comparison",
            _ => continue,
        };
        binding.value = openengine_cluster_protocol::DataSelector::State {
            path: FieldPath::new(vec![
                openengine_cluster_protocol::FieldName::new(path).assert_value(),
            ])
            .assert_value(),
        };
    }
    admitted.initial_input = json!({
        "destination":admitted.initial_input["url"],
        "comparison":admitted.initial_input["context"]["reviewScope"]
    });
    assert_eq!(
        checkout_references(&admitted).assert_value(),
        ["refs/pull/17/head".to_owned(), "b".repeat(40)]
    );
    let source = admitted.source.take().assert_value();
    assert!(checkout_references(&admitted).assert_value().is_empty());
    admitted.source = Some(source);
    admitted.source.as_mut().unwrap().revision =
        SourceRevisionId::new("c".repeat(40)).assert_value();
    assert!(matches!(
        checkout_references(&admitted),
        Err(ReviewResolveError::Identity)
    ));
}
