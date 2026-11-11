use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn missing_script_program_is_named_in_cli_log_replay_without_arguments_or_environment() {
    let fixture = LocalFixture::new();
    let program = "zeroshot-missing-script-program-1519";
    let argument = "private-script-argument";
    let mut graph = local_graph::graph();
    graph["root"]["children"][0]
        .as_object_mut()
        .assert_value()
        .remove("instructions");
    write_json(&fixture.graph, &graph);
    let mut runtime = local_runtime();
    runtime["nodes"]["worker"] = json!({
        "kind":"script",
        "command":[program, argument],
        "connections":{"openai":["OPENAI_API_KEY"]}
    });
    write_json(&fixture.runtime, &runtime);

    let run_id = fixture.submit_detached("finish").await;
    wait_for_exit(fixture.ready_pid(&run_id)).await;
    let logs = fixture.output(&["logs", &run_id], "finish").await;
    assert_success(&logs, "failed script log replay");
    let text = String::from_utf8_lossy(&logs.stdout);
    assert!(text.contains("script launch failed"), "{text}");
    assert!(text.contains("process spawn failed"), "{text}");
    assert!(text.contains(program), "{text}");
    for private in [argument, DECLARED_KEY, "must-not-cross"] {
        assert!(!text.contains(private), "{text}");
    }
}
