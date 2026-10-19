use super::*;

#[tokio::test]
async fn review_and_delivery_repairs_share_the_budget_and_worker_instance() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Merge] {
        let (verified, input) = verified_software_template(delivery).await;
        let mut history = Vec::new();
        let mut expected_phase = json!({});
        let mut delivery_feedback = String::new();
        for round in 0..10 {
            let next = reduce(&verified, &input, &history);
            assert_dispatched_together(&next, &["worker"]);
            let execution = history.len() as u64 + 1;
            settle_worker_visit(&next, &mut history, &expected_phase);
            assert_dispatched_together(
                &reduce(&verified, &input, &history),
                &["acceptance", "code"],
            );
            let review_input =
                json!({"task":"repair checkout", "deliveryFeedback":delivery_feedback});
            history.push(settled_review_execution_with_output(
                SettledExecutionSpec {
                    execution: execution + 1,
                    node_instance: 2,
                    node: "acceptance",
                    settled_at: execution + 1,
                    input: review_input.clone(),
                },
                ACCEPTED_LABEL,
                "requirements met",
                if delivery == TemplateDelivery::None {
                    Value::Null
                } else {
                    change_manifest()
                },
            ));
            // A previous round's code approval must not admit delivery while this review is pending.
            assert_dispatched_together(&reduce(&verified, &input, &history), &["code"]);
            let rejected = delivery == TemplateDelivery::None || round % 2 == 0;
            history.push(settled_review_execution(
                SettledExecutionSpec {
                    execution: execution + 2,
                    node_instance: 3,
                    node: "code",
                    settled_at: execution + 2,
                    input: review_input,
                },
                if rejected {
                    REJECTED_LABEL
                } else {
                    ACCEPTED_LABEL
                },
                "code feedback",
            ));
            expected_phase = json!({"after":"accepted"});
            if !rejected {
                assert_dispatched_together(&reduce(&verified, &input, &history), &[DELIVERY_NODE]);
                let outcome = [
                    DELIVERY_CI_FAILED_LABEL,
                    DELIVERY_CONFLICT_LABEL,
                    DELIVERY_REPAIR_REQUIRED_LABEL,
                ][(round / 2) % 3];
                delivery_feedback = format!("delivery repair {round}");
                history.push(settled_delivery_with_diagnostic(
                    SettledExecutionSpec {
                        execution: execution + 3,
                        node_instance: 4,
                        node: DELIVERY_NODE,
                        settled_at: execution + 3,
                        input: delivery_input(),
                    },
                    DeliveryMode::MergeV3,
                    outcome,
                    &delivery_feedback,
                ));
                expected_phase = json!({"after":outcome});
            }
        }
        let exhausted = reduce(&verified, &input, &history);
        assert_eq!(
            exhausted.terminal,
            Some(TerminalProjection::Failed {
                context: None,
                reason: "change_attempts_exhausted".parse().assert_value(),
            })
        );
        assert_dispatched_together(&exhausted, &[]);
    }
}

fn settle_worker_visit(
    reduction: &Reduction,
    history: &mut Vec<DurableExecution>,
    expected_phase: &Value,
) {
    let worker_input = reduction
        .decisions
        .iter()
        .find_map(|decision| match decision {
            Decision::Dispatch {
                node_instance,
                occurrence,
                input,
                ..
            } if occurrence.node.as_str() == "worker" => {
                assert_eq!(
                    node_instance.get(),
                    1,
                    "every repair revisits the initial worker"
                );
                Some(input.clone())
            }
            _ => None,
        })
        .assert_value();
    assert_eq!(&worker_input["phase"], expected_phase);
    let execution = history.len() as u64 + 1;
    history.push(settled_agent(SettledExecutionSpec {
        execution,
        node_instance: 1,
        node: "worker",
        settled_at: execution,
        input: worker_input,
    }));
}
