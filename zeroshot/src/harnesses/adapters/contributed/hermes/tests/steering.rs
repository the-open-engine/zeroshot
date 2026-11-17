#![cfg(unix)]

use super::*;

#[tokio::test]
async fn steer_hermes_reassembles_aggregate_prompts_beyond_one_native_frame() {
    // Exercise the shipped control-frame reader, with the fixture standing in for the
    // native model call. Chunk frames must never become separate native turns.
    let source = fixture("{\"response\":null}", true)
        .replace(
            "for line in sys.stdin:\n    message=json.loads(line)",
            r#"import importlib.util, threading
spec=importlib.util.spec_from_file_location('owned_runtime', os.environ['ZEROSHOT_HERMES_BRIDGE'])
bridge_module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge_module)
bridge=bridge_module.Runtime(lambda *_args, **_kwargs: None)
bridge.interrupt=lambda: None
threading.Thread(target=bridge.controls, args=(sys.stdin.buffer,), daemon=True).start()
def messages():
    while not bridge.stop.is_set():
        try: text=bridge.commands.get(timeout=0.1)
        except bridge_module.queue.Empty: continue
        yield {'type':'prompt','text':text}
    assert not bridge.control_failed
    yield {'type':'stop'}
for message in messages():"#,
        )
        .replace(
            "assert message['type']=='prompt'",
            "assert message['type']=='prompt'; pathlib.Path('received-prompt.txt').write_text(message['text'], encoding='utf-8'); pathlib.Path('turns').open('a').write('turn\\n')",
        );
    let directory = TestDirectory::new("hermes-steer-prompt");
    let adapter = adapter(&directory, &source);
    let mut invocation = invocation(&[]);
    let mut context = String::from("<steer>\n");
    for i in 0..256 {
        context.push_str(&format!(
            "[steer-{i}] {i:03}{}x\n",
            "🦀".repeat((openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 4) / 4)
        ));
    }
    // Exercise JSON expansion as well as UTF-8 boundaries between private frames.
    context.push_str(&"\t\r\n\"\\".repeat(1024));
    context.push_str("\n</steer>");
    invocation.host_context = Some(context);
    let expected = invocation
        .agent_prompt_with_schema(crate::native_v2_runner::ProviderSchemaDialect::Standard)
        .assert_value();
    assert!(expected.len() > 64 * 1024 * 1024);
    assert!(matches!(
        run(&adapter, invocation).await.assert_value(),
        WorkerOutcome::Verified { .. }
    ));
    let received =
        std::fs::read(adapter.config.workspace.join("received-prompt.txt")).assert_value();
    assert_eq!(received.len(), expected.len());
    assert!(
        received == expected.as_bytes(),
        "the complete prompt must cross the private bridge unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(adapter.config.workspace.join("turns")).assert_value(),
        "turn\n"
    );
}
