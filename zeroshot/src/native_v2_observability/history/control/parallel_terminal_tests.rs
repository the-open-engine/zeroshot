use super::*;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_contract::RunSubmission;
use crate::v2_run_ledger::{RunSnapshot, cursor_for};
use openengine_cluster_protocol::{IdempotencyKey, RunTitle, Sha256Digest};
use openengine_cluster_testkit::assertions::AssertValue;
use openengine_cluster_testkit::fixture::run_size;

#[tokio::test]
async fn parallel_terminal_ui_fixture_matches_canonical_control_replay() {
    // Shared with the browser regression: keep native cursor/visit identities authoritative.
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ui/src/fixtures/parallel-terminal.json"
    )))
    .assert_value();
    for join in [
        json!({"kind":"all"}),
        json!({"kind":"any"}),
        json!({"kind":"quorum","count":1}),
        json!({"kind":"first","when":{"kind":"in",
            "value":{"source":"signal","name":"docs","field":"verdict"},
            "labels":["accepted"]}}),
    ] {
        for succeeds in [false, true] {
            let mut graph = fixture["graph"].clone();
            let parallel = &mut graph["root"]["children"][0]["body"];
            parallel["join"] = join.clone();
            let mut expected = fixture["control"].clone();
            if succeeds {
                parallel["branches"][0] = json!({"kind":"succeed","name":"blocked",
                    "output":{"kind":"null"},"bindings":[]});
                expected[3] = json!({"cursor":"v2:1","node":"blocked","mapIndices":[],
                    "visitId":"[\"blocked\",[],[1]]","state":"succeeded","output":null});
            }
            let admitted = NativeV2Admission
                .admit(RunSubmission {
                    environment: None,
                    title: RunTitle::new("Parallel terminal replay").assert_value(),
                    graph: serde_json::from_value(graph).assert_value(),
                    initial_input: json!({}),
                    runtime: crate::native_v2_candidate::test_support::codex_runtime(
                        run_size("small"),
                        BTreeMap::from([(
                            "docs".parse().assert_value(),
                            crate::native_v2_runner::test_support::binding(
                                crate::execution::SessionScope::Execution,
                            ),
                        )]),
                    ),
                    source: None,
                    submission_key: IdempotencyKey::new("parallel-terminal").assert_value(),
                })
                .await
                .assert_value();
            let snapshot =
                RunSnapshot::admitted(RunId::new("parallel-terminal"), &admitted).assert_value();
            let mut replay = ControlReplay::new(StoredRun {
                submission_key: IdempotencyKey::new("parallel-terminal").assert_value(),
                submission_digest: Sha256Digest::new("0".repeat(64)).assert_value(),
                admitted,
                snapshot,
            });
            replay
                .apply(StoredRunEvent {
                    recorded_at: None,
                    cursor: cursor_for(1),
                    event: RunEvent::RunStarted,
                })
                .assert_value();
            let page = replay.page(0..1);
            assert_eq!(page.error, None);
            assert_eq!(serde_json::to_value(page.records).assert_value(), expected);
        }
    }
}
