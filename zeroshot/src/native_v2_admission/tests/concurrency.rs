use super::*;

fn state() -> Value {
    json!({"kind":"record","fields":{
        "items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}
    }})
}

fn writer() -> Value {
    null_step("writer", "agent.writer@1")
}

fn delivery() -> Value {
    delivery_verifier("deliver", DeliveryMode::PullRequest)
}

fn parallel(left: Value, right: Value) -> Value {
    json!({"kind":"par","name":"parallel","state":state(),
        "branches":[left,right],"promotedStatePaths":[],"join":{"kind":"all"}})
}

fn sequence(children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":"sequence","state":state(),
        "children":children,"promotedStatePaths":[]})
}

fn mapped(name: &str, maximum: u64, body: Value) -> Value {
    json!({"kind":"map","name":name,"state":state(),"body":body,
        "over":{"source":"state","path":["items"]},"maxItems":maximum,
        "promotedStatePaths":[]})
}

fn looped(body: Value) -> Value {
    json!({"kind":"loop","name":"loop","state":state(),"body":body,
        "maxIterations":2,"promotedStatePaths":[]})
}

fn choice(branch: Value, otherwise: Option<Value>) -> Value {
    json!({"kind":"choice","name":"choice","state":state(),"branches":[{
        "when":{"kind":"in","value":{"name":"reader","source":"signal","field":"verdict"},
            "labels":["accepted"]},"node":branch
    }],"otherwise":otherwise,"promotedStatePaths":[]})
}

fn request(node: Value) -> RunSubmission {
    let graph = graph(vec![
        null_verifier("reader", "agent.reader@1"),
        node,
        succeed("done"),
    ]);
    let nodes = executable_declarations(&graph.root)
        .into_iter()
        .map(|declaration| {
            let binding = if declaration.name == named("deliver") {
                delivery_binding()
            } else {
                binding("claude-sonnet-5", None)
            };
            (declaration.name, binding)
        })
        .collect();
    submission(graph, nodes)
}

async fn assert_rejected(request: RunSubmission, expected: NativeV2AdmissionError) {
    for policy in [DeliveryPolicy::Optional, DeliveryPolicy::Required] {
        assert_eq!(
            NativeV2Admission
                .validate_profile(&request.graph, &request.runtime, policy)
                .await,
            Err(expected.clone()),
        );
        assert_eq!(
            NativeV2Admission
                .validate_intent(&submission_intent(&request), policy)
                .await,
            Err(expected.clone()),
        );
        assert_eq!(
            NativeV2Admission
                .admit_with_policy(request.clone(), policy)
                .await,
            Err(expected.clone()),
        );
    }
}

#[tokio::test]
async fn rejects_delivery_parallel_with_writers_through_nested_groups() {
    for node in [
        parallel(delivery(), writer()),
        parallel(writer(), delivery()),
        parallel(looped(delivery()), choice(writer(), None)),
        parallel(
            choice(null_verifier("other", "agent.other@1"), Some(delivery())),
            writer(),
        ),
        parallel(sequence(vec![delivery()]), mapped("each", 2, writer())),
    ] {
        assert_rejected(
            request(node),
            NativeV2AdmissionError::ConcurrentDelivery {
                delivery: named("deliver"),
                writer: named("writer"),
            },
        )
        .await;
    }
}

#[tokio::test]
async fn rejects_delivery_repeated_by_concurrent_maps_through_nested_groups() {
    for body in [
        delivery(),
        looped(delivery()),
        choice(delivery(), None),
        choice(null_verifier("other", "agent.other@1"), Some(delivery())),
        sequence(vec![writer(), delivery()]),
        mapped("inner", 1, delivery()),
    ] {
        assert_rejected(
            request(mapped("each", 2, body)),
            NativeV2AdmissionError::ConcurrentMapDelivery {
                map: named("each"),
                delivery: named("deliver"),
            },
        )
        .await;
    }
    assert_rejected(
        request(mapped("outer", 1, mapped("inner", 2, delivery()))),
        NativeV2AdmissionError::ConcurrentMapDelivery {
            map: named("inner"),
            delivery: named("deliver"),
        },
    )
    .await;
}

#[tokio::test]
async fn admits_delivery_after_joined_writers_and_in_sequential_groups() {
    for node in [
        sequence(vec![
            parallel(writer(), null_step("other", "agent.other@1")),
            delivery(),
        ]),
        sequence(vec![mapped("each", 2, writer()), delivery()]),
        mapped("each", 1, delivery()),
        mapped("outer", 1, mapped("inner", 1, delivery())),
        looped(sequence(vec![writer(), delivery()])),
        choice(writer(), Some(delivery())),
        parallel(delivery(), null_verifier("other", "agent.other@1")),
    ] {
        assert_concurrent_admission(request(node), DeliveryPolicy::Required).await;
    }
}
