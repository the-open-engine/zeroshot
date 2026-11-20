use super::*;
use crate::native_v2_contract::{ExecutionId, NodeInstanceId};
use crate::v2_run_ledger::vector_event;
use openengine_cluster_protocol::{NodeName, RunVectorContent, SteeringMode};

fn turn(node: &str, instance: u64, execution: u64, home: u64) -> DriverInvocation {
    let mut turn = invocation();
    turn.node.reference.node = NodeName::new(node).assert_value();
    turn.node.reference.node_instance = NodeInstanceId::new(instance).assert_value();
    turn.node.reference.execution = ExecutionId::new(execution).assert_value();
    turn.provider_home_slot = NodeInstanceId::new(home).assert_value();
    turn
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_node_filters_prompts_and_hooks_for_mapped_instances_and_fresh_conversations() {
    for mode in [SteeringMode::Once, SteeringMode::Standing] {
        for in_prompt in [false, true] {
            let (ledger, run) = steer_record().await;
            let hooks = ActiveMemoryHooks::empty().assert_value();
            hooks.bind_ledger(run.clone(), ledger.clone());
            let mut turns = [
                turn("worker", 7, 11, 17),
                turn("unrelated", 8, 19, 23),
                turn("worker", 9, 29, 31),
            ];
            if !in_prompt {
                for turn in &mut turns {
                    hooks.prepare_steers(turn, false).await.assert_value();
                }
            }
            // Publication admission is covered through the CLI; isolate recipient selection here.
            ledger
                .append(
                    &run,
                    vec![
                        vector_event(
                            &run,
                            RunVectorContent::Steer {
                                text: "selected-node-direction".into(),
                                mode,
                                network: None,
                                node: Some(NodeName::new("worker").assert_value()),
                            },
                        )
                        .assert_value(),
                    ],
                )
                .await
                .assert_value();
            for turn in &mut turns {
                let matching = turn.node.reference.node.as_str() == "worker";
                if in_prompt {
                    hooks.prepare_steers(turn, false).await.assert_value();
                    assert_eq!(
                        turn.host_context
                            .as_deref()
                            .unwrap_or_default()
                            .contains("selected-node-direction"),
                        matching
                    );
                } else {
                    let (query, delivery) = hooks
                        .tool_delivery(ToolCall::hook(
                            turn,
                            &json!({"tool_name":"Read"}),
                            Path::new("/workspace"),
                        ))
                        .await;
                    assert_eq!(
                        delivery.context.contains("selected-node-direction"),
                        matching
                    );
                    assert_eq!(delivery.steers.len(), usize::from(matching));
                    hooks.acknowledge(&query, &delivery).await;
                    let (_, repeat) = hooks
                        .tool_delivery(ToolCall::hook(
                            turn,
                            &json!({"tool_name":"Read"}),
                            Path::new("/workspace"),
                        ))
                        .await;
                    assert!(repeat.steers.is_empty());
                }
                hooks
                    .finish_steers(turn.node.reference.execution.get(), true)
                    .await;
            }
            // A new conversation for an existing instance consumes Once, but receives Standing.
            // A later mapped copy or visit has a fresh instance and receives either mode.
            for (mut turn, expected) in [
                (turn("worker", 7, 37, 41), mode == SteeringMode::Standing),
                (turn("worker", 10, 43, 47), true),
                (turn("unrelated", 12, 53, 59), false),
            ] {
                hooks.prepare_steers(&mut turn, false).await.assert_value();
                assert_eq!(
                    turn.host_context
                        .as_deref()
                        .unwrap_or_default()
                        .contains("selected-node-direction"),
                    expected
                );
                hooks
                    .finish_steers(turn.node.reference.execution.get(), true)
                    .await;
            }
            let status = crate::v2_run_ledger::steering::status(ledger.as_ref(), &run)
                .await
                .assert_value();
            assert_eq!(
                status.steers[0].received_executions,
                if mode == SteeringMode::Standing { 4 } else { 3 }
            );
            assert!(
                crate::v2_run_ledger::steering::all(ledger.as_ref(), &run)
                    .await
                    .assert_value()
                    .iter()
                    .all(|vector| !matches!(
                        &vector.content,
                        RunVectorContent::SteerReceipt {
                            node_instance: 8 | 12,
                            ..
                        }
                    ))
            );
        }
    }
}
