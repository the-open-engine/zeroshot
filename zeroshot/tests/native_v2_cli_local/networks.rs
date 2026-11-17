use super::*;

const ONCE: &str = "peer-once-marker";
const STANDING: &str = "peer-standing-marker";

fn network_step(name: &str) -> Value {
    let mut node = local_graph::graph()["root"]["children"][0].clone();
    node["name"] = json!(name);
    node["worker"] = json!(format!("agent.{name}@1"));
    node["instructions"] = json!("Exercise the declared peer network and return null.");
    node
}

fn network_fixture(scenario: &str, parallel: bool) -> LocalFixture {
    let mut fixture = LocalFixture::new();
    fixture.working_directory = fixture.root.as_path().into();
    fixture.cwd = Some(fixture.repository.clone());
    let mut graph = local_graph::graph();
    let members = if parallel {
        json!(["talker", "receiver"])
    } else {
        json!(["sender", "receiver"])
    };
    let publishers = if parallel {
        json!(["talker"])
    } else {
        json!(["sender"])
    };
    graph["networks"] = json!({
        "alpha":{"members":members,"publishers":publishers},
        "beta":{"members":members,"publishers":publishers}
    });
    graph["root"]["children"] = if parallel {
        let state = json!({"kind":"record","fields":{
            "items":{"type":{"kind":"array","items":{"kind":"integer"}},"required":true}
        }});
        graph["initialInput"] = state.clone();
        graph["root"]["state"] = state.clone();
        write_json(&fixture.input, &json!({"items":[0,1]}));
        json!([
            {"kind":"map","name":"peers","state":state,"body":network_step("talker"),
             "over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[]},
            network_step("receiver"), network_step("outsider"),
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ])
    } else {
        json!([
            {"kind":"loop","name":"twice","state":{"kind":"null"},"maxIterations":2,
             "promotedStatePaths":[],"body":{"kind":"seq","name":"round","state":{"kind":"null"},
             "promotedStatePaths":[],"children":[network_step("sender"),network_step("receiver")]}},
            network_step("outsider"),
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ])
    };
    write_json(&fixture.graph, &graph);
    let binding = json!({"kind":"agent","model":"fixture-model","sessionScope":"execution",
        "connections":{"fixture":["ANTHROPIC_API_KEY","NETWORK_CLI","NETWORK_SCENARIO"]}});
    let names: &[&str] = if parallel {
        &["talker", "receiver", "outsider"]
    } else {
        &["sender", "receiver", "outsider"]
    };
    let nodes: serde_json::Map<_, _> = names
        .iter()
        .map(|name| ((*name).to_owned(), binding.clone()))
        .collect();
    write_json(
        &fixture.runtime,
        &json!({"harness":"claude","provider":"anthropic","size":"host",
        "workspaceCheckpoints":scenario == "resume", "activeMemory":{"repository":false,"graph":false},
        "nodes":nodes}),
    );
    let home = fixture.root.path("network-provider-home");
    fs::create_dir_all(&home).assert_value();
    fixture.harness_environment.extend([
        ("HOME".into(), home.display().to_string()),
        ("CLAUDE_CONFIG_DIR".into(), home.display().to_string()),
        ("ANTHROPIC_API_KEY".into(), DECLARED_KEY.into()),
        (
            "NETWORK_CLI".into(),
            fixture.executable.display().to_string(),
        ),
        ("NETWORK_SCENARIO".into(), scenario.into()),
    ]);
    let helper = fixture.root.path("network-provider.py");
    openengine_cluster_testkit::fixture::write_executable(
        &helper,
        openengine_cluster_testkit::fixture::python3_script(NETWORK_PROVIDER).as_bytes(),
        0o755,
    )
    .assert_value();
    let wrapper = format!(
        "#!/bin/sh\nexec {} \"$@\"\n",
        shell_literal(helper.to_str().assert_value())
    );
    openengine_cluster_testkit::fixture::write_executable(
        &fixture.root.path("bin/claude"),
        with_configuration_probe(
            wrapper.as_bytes(),
            "claude",
            &ProbeResponse::Configured(json!({})),
        )
        .as_bytes(),
        0o755,
    )
    .assert_value();
    fixture
}

const NETWORK_PROVIDER: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys, time
root = pathlib.Path.cwd()
run, node, iteration, instance = [os.environ[k] for k in
    ('ZEROSHOT_RUN_ID','ZEROSHOT_NODE_NAME','ZEROSHOT_NODE_ITERATION','ZEROSHOT_NODE_INSTANCE_ID')]
scenario = os.environ['NETWORK_SCENARIO']
identity = run+'-'+node+'-'+iteration+'-'+instance

def save(kind, value):
    (root/('network-'+identity+'-'+kind)).write_text(value if isinstance(value,str) else json.dumps(value))

def wait(predicate):
    deadline = time.monotonic()+15
    while not predicate():
        if time.monotonic() >= deadline: raise RuntimeError('network fixture barrier timed out')
        time.sleep(.01)

save('prompt', sys.stdin.read())
save('launch', sys.argv[1:])
save('capability', bool(os.environ.get('ZEROSHOT_STEERING_CAPABILITY')))
save('identity', {'node':node,'nodeInstance':int(instance),'execution':os.environ['ZEROSHOT_EXECUTION_REF']})
settings = next((arg.split('=',1)[1] if arg.startswith('--settings=') else sys.argv[i+1])
    for i,arg in enumerate(sys.argv[1:],1) if arg == '--settings' or arg.startswith('--settings='))
handler = json.loads(settings)['hooks']['PreToolUse'][0]['hooks'][0]['command']

def tool(name):
    result = subprocess.run(handler, shell=True, input=json.dumps({'tool_name':'Bash','tool_input':{'command':'printf peer-proof'}}),
        text=True, capture_output=True, timeout=10)
    save(name, result.stdout)
    if result.returncode: raise RuntimeError('native carrier failed: '+result.stderr)

def publish(name, text, args=(), override=None, target_run=None, stdin=None):
    environment = os.environ.copy()
    if override: environment.update(override)
    result = subprocess.run([os.environ['NETWORK_CLI'],'steer',target_run or run,*args,*([text] if text is not None else [])],
        env=environment,input=stdin,text=True,capture_output=True,timeout=10)
    save(name, {'status':result.returncode,'stdout':result.stdout,'stderr':result.stderr})
    return result.returncode

response = None
if scenario == 'rollback' and node == 'chooser':
    route = 'next' if (root/'network-rolled-back').exists() else 'first'
    response = {'output':None,'signals':{'route':route},'diagnostic':None}
elif scenario == 'default-deny':
    assert publish('deny-network','forbidden-undeclared',('--network','alpha')) != 0
    assert publish('deny-broadcast','forbidden-agent-broadcast') != 0
elif node == 'sender' and (iteration == '1' or scenario == 'rollback'):
    if scenario == 'deny':
        (root/'network-host-held').touch()
        wait(lambda: (root/'network-host-release').exists())
    assert publish('publish-once','peer-once-marker',('--network','alpha','--network','beta'),
        {'ZEROSHOT_NODE_NAME':'outsider','ZEROSHOT_NODE_INSTANCE_ID':'65536','ZEROSHOT_EXECUTION_REF':'forged'}) == 0
    standing_file = root/'network-standing-message.txt'
    standing_file.write_text('peer-standing-marker')
    assert publish('publish-standing',None,('--network','alpha','--network','beta','--mode','standing',
        '--text-file',str(standing_file))) == 0
    if scenario == 'deny':
        assert publish('deny-foreign-run','forbidden-foreign',('--network','alpha'),
            target_run='0199f33f-3b44-7d21-9000-000000000099') != 0
        assert publish('deny-mixed-networks','forbidden-mixed',('--network','alpha','--network','undeclared')) != 0
        assert publish('deny-broadcast','forbidden-broadcast') != 0
        assert publish('deny-missing-capability','forbidden-without-capability',('--network','alpha'),
            {'ZEROSHOT_STEERING_CAPABILITY':''}) != 0
    if scenario == 'rollback': (root/'network-rolled-back').touch()
elif node == 'talker':
    tool('tool-before')
    (root/('network-ready-'+instance)).touch()
    wait(lambda: len(list(root.glob('network-ready-*'))) == 2)
    assert publish('publish-once',None,('--network','alpha','--network','beta','--text-stdin'),
        stdin='mapped-once-'+instance) == 0
    assert publish('publish-standing','mapped-standing-'+instance,('--network','alpha','--network','beta','--mode','standing')) == 0
    (root/('network-published-'+instance)).touch()
    wait(lambda: len(list(root.glob('network-published-*'))) == 2)
elif node in ('receiver','outsider') and scenario == 'deny':
    assert publish('deny-network','forbidden-'+node,('--network','alpha')) != 0
    assert publish('deny-spoof','forbidden-spoof-'+node,('--network','alpha'),
        {'ZEROSHOT_NODE_NAME':'sender','ZEROSHOT_NODE_INSTANCE_ID':'1','ZEROSHOT_EXECUTION_REF':'forged'}) != 0

tool('tool-0')
tool('tool-1')
if scenario == 'resume' and node == 'receiver' and iteration == '1':
    (root/'network-resume-held').touch()
    wait(lambda: (root/'network-resume-release').exists())
print(json.dumps({'type':'system','subtype':'init','session_id':'network-'+identity}))
print(json.dumps({'type':'result','subtype':'success','is_error':False,'session_id':'network-'+identity,
    'structured_output':{'response':response},'model_usage_since_last_result':{'fixture-model':{
        'inputTokens':1,'outputTokens':1,'cacheReadInputTokens':0,'cacheCreationInputTokens':0}}}))
"#;

fn network_captures(
    fixture: &LocalFixture,
    run: &str,
    (node, iteration): (&str, u64),
    kind: &str,
) -> Vec<String> {
    let prefix = format!("network-{run}-{node}-{iteration}-");
    let suffix = format!("-{kind}");
    let mut paths = fs::read_dir(&fixture.repository)
        .assert_value()
        .map(|entry| entry.assert_value().path())
        .filter(|path| {
            path.file_name()
                .assert_value()
                .to_string_lossy()
                .starts_with(&prefix)
                && path
                    .file_name()
                    .assert_value()
                    .to_string_lossy()
                    .ends_with(&suffix)
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| fs::read_to_string(path).assert_value())
        .collect()
}

fn network_capture(
    fixture: &LocalFixture,
    run: &str,
    (node, iteration): (&str, u64),
    kind: &str,
) -> String {
    let captures = network_captures(fixture, run, (node, iteration), kind);
    assert_eq!(
        captures.len(),
        1,
        "unexpected launches for {node}/{iteration}/{kind}"
    );
    captures[0].clone()
}

async fn network_wait(fixture: &LocalFixture, marker: &str) {
    timeout(CLI_TIMEOUT, async {
        loop {
            if fixture.repository.join(marker).exists() {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .assert_value_with(marker);
}

fn network_tool_context(text: &str) -> String {
    let tool: Value = serde_json::from_str(text).assert_value();
    tool["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn network_received(fixture: &LocalFixture, run: &str, node: &str, iteration: u64) -> String {
    ["prompt", "tool-0", "tool-1"]
        .map(|kind| network_capture(fixture, run, (node, iteration), kind))
        .join("\n")
}

fn network_no_messages(fixture: &LocalFixture, run: &str, node: &str, iteration: u64) {
    for kind in ["prompt", "tool-0", "tool-1"] {
        let text = network_capture(fixture, run, (node, iteration), kind);
        assert!(
            !text.contains(ONCE) && !text.contains(STANDING),
            "self or outsider delivery: {text}"
        );
    }
}

fn network_two_message_receipts(status: &Value) {
    let counts = status["steers"]
        .as_array()
        .assert_value()
        .iter()
        .map(|steer| steer["receivedExecutions"].as_u64().assert_value())
        .collect::<Vec<_>>();
    assert_eq!(counts, [1, 2], "once and standing receipts: {status}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_sequential_default_once_standing_and_overlap_keep_sender_and_outsider_excluded() {
    let fixture = network_fixture("sequential", false);
    let run = fixture.submit_detached("networks").await;
    fixture.wait_succeeded(&run, "networks").await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    for iteration in [1, 2] {
        network_no_messages(&fixture, &run, "sender", iteration);
        let prompt = network_capture(&fixture, &run, ("receiver", iteration), "prompt");
        let received = network_received(&fixture, &run, "receiver", iteration);
        assert_eq!(received.matches(ONCE).count(), usize::from(iteration == 1));
        assert_eq!(received.matches(STANDING).count(), 1);
        assert_eq!(
            network_capture(&fixture, &run, ("sender", iteration), "capability"),
            "true"
        );
        let publisher = network_capture(&fixture, &run, ("sender", iteration), "prompt");
        assert!(publisher.contains("<steering-networks>"));
        assert!(publisher.contains("alpha") && publisher.contains("beta"));
        assert!(!prompt.contains("<steering-networks>"));
        assert_eq!(
            network_capture(&fixture, &run, ("receiver", iteration), "capability"),
            "false"
        );
        let launch: Vec<String> = serde_json::from_str(&network_capture(
            &fixture,
            &run,
            ("receiver", iteration),
            "launch",
        ))
        .assert_value();
        assert!(
            !launch.iter().any(|arg| arg == "--resume"),
            "each receiver must start fresh"
        );
    }
    network_no_messages(&fixture, &run, "outsider", 0);
    assert_eq!(
        network_capture(&fixture, &run, ("outsider", 0), "capability"),
        "false"
    );
    let status = fixture.json(&["status", &run], "networks").await;
    let steers = status["steers"].as_array().assert_value();
    assert_eq!(steers.len(), 2);
    let source: Value =
        serde_json::from_str(&network_capture(&fixture, &run, ("sender", 1), "identity"))
            .assert_value();
    assert_eq!(
        steers[0]["network"]["origin"], source,
        "the host must assign the origin independently of caller labels"
    );
    assert_eq!(steers[0]["mode"], "once");
    assert_eq!(steers[1]["mode"], "standing");
    assert_eq!(steers[0]["network"]["networks"], json!(["alpha", "beta"]));
    network_two_message_receipts(&status);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_map_peers_deliver_at_live_hooks_once_per_sibling_and_future_member_without_waking()
 {
    let fixture = network_fixture("parallel", true);
    let run = fixture.submit_detached("networks").await;
    fixture.wait_succeeded(&run, "networks").await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    let prompts = network_captures(&fixture, &run, ("talker", 0), "prompt");
    assert_eq!(
        prompts.len(),
        2,
        "only the two authored mapped instances may start"
    );
    let messages = network_captures(&fixture, &run, ("talker", 0), "publish-once")
        .into_iter()
        .map(|publication| {
            let publication: Value = serde_json::from_str(&publication).assert_value();
            let result: Value =
                serde_json::from_str(publication["stdout"].as_str().assert_value()).assert_value();
            result["steer"]["excerpt"]
                .as_str()
                .assert_value()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    for prompt in prompts {
        assert!(!messages.iter().any(|message| prompt.contains(message)));
    }
    let tools = network_captures(&fixture, &run, ("talker", 0), "tool-0");
    let later = network_captures(&fixture, &run, ("talker", 0), "tool-1");
    assert_eq!(tools.len(), 2);
    assert_eq!(later.len(), tools.len());
    let tools = tools
        .into_iter()
        .zip(later)
        .map(|(first, second)| {
            format!(
                "{}\n{}",
                network_tool_context(&first),
                network_tool_context(&second)
            )
        })
        .collect::<Vec<_>>();
    let identities = network_captures(&fixture, &run, ("talker", 0), "identity");
    assert_eq!(identities.len(), tools.len());
    for (tool, identity) in tools.into_iter().zip(identities) {
        let identity: Value = serde_json::from_str(&identity).assert_value();
        let own_instance = identity["nodeInstance"].as_u64().assert_value();
        let context = tool;
        assert!(!context.contains(&format!("mapped-once-{own_instance}\n")));
        assert!(!context.contains(&format!("mapped-standing-{own_instance}\n")));
        assert_eq!(context.matches("mapped-once-").count(), 1);
        assert_eq!(context.matches("mapped-standing-").count(), 1);
    }
    for tool in network_captures(&fixture, &run, ("talker", 0), "tool-before") {
        assert!(!tool.contains("mapped-once-") && !tool.contains("mapped-standing-"));
    }
    let receiver = network_received(&fixture, &run, "receiver", 0);
    for message in messages {
        assert_eq!(receiver.matches(&message).count(), 1);
    }
    assert_eq!(receiver.matches("mapped-standing-").count(), 2);
    let outsider = network_received(&fixture, &run, "outsider", 0);
    assert!(!outsider.contains("mapped-once-") && !outsider.contains("mapped-standing-"));
    let status = fixture.json(&["status", &run], "networks").await;
    let steers = status["steers"].as_array().assert_value();
    assert_eq!(steers.len(), 4);
    for steer in steers {
        assert_eq!(steer["receivedExecutions"], 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_denied_host_member_and_spoofed_publisher_calls_append_no_messages() {
    let fixture = network_fixture("deny", false);
    let run = fixture.submit_detached("networks").await;
    network_wait(&fixture, "network-host-held").await;
    let denied = fixture
        .output(
            &["steer", &run, "--network", "alpha", "forbidden-host"],
            "networks",
        )
        .await;
    assert!(!denied.status.success());
    assert!(
        fixture.json(&["vectors", &run], "networks").await["vectors"]
            .as_array()
            .assert_value()
            .is_empty()
    );
    fs::write(fixture.repository.join("network-host-release"), "go").assert_value();
    fixture.wait_succeeded(&run, "networks").await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    for (node, iteration, kinds) in [
        (
            "sender",
            1,
            &[
                "deny-broadcast",
                "deny-missing-capability",
                "deny-foreign-run",
                "deny-mixed-networks",
            ][..],
        ),
        ("receiver", 1, &["deny-network", "deny-spoof"][..]),
        ("outsider", 0, &["deny-network", "deny-spoof"][..]),
    ] {
        for kind in kinds {
            let output: Value =
                serde_json::from_str(&network_capture(&fixture, &run, (node, iteration), kind))
                    .assert_value();
            assert_ne!(output["status"], 0, "{output}");
        }
    }
    let vectors = fixture.json(&["vectors", &run], "networks").await;
    assert!(!vectors.to_string().contains("forbidden-"));
    assert_eq!(
        vectors["vectors"]
            .as_array()
            .assert_value()
            .iter()
            .filter(|v| v["kind"] == "steer")
            .count(),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_suspended_successor_keeps_once_consumption_and_sender_exclusion_in_fresh_sessions()
 {
    let fixture = network_fixture("resume", false);
    let run = fixture.submit_detached("networks").await;
    network_wait(&fixture, "network-resume-held").await;
    fixture.json(&["suspend", &run], "networks").await;
    fs::write(fixture.repository.join("network-resume-release"), "go").assert_value();
    fixture.wait_suspended(&run).await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    let next = fixture.json(&["resume", &run], "networks").await;
    let next = next["runId"].as_str().assert_value();
    fixture.wait_succeeded(next, "networks").await;
    wait_for_exit(fixture.ready_pid(next)).await;
    network_no_messages(&fixture, next, "sender", 2);
    let receiver = network_received(&fixture, next, "receiver", 2);
    assert!(!receiver.contains(ONCE), "{receiver}");
    assert_eq!(receiver.matches(STANDING).count(), 1, "{receiver}");
    let launch: Vec<String> =
        serde_json::from_str(&network_capture(&fixture, next, ("receiver", 2), "launch"))
            .assert_value();
    assert!(!launch.iter().any(|arg| arg == "--resume"));
    let status = fixture.json(&["status", next], "networks").await;
    network_two_message_receipts(&status);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_omitted_declarations_deny_every_agent_publication_and_publish_context() {
    let fixture = network_fixture("default-deny", false);
    let mut graph: Value =
        serde_json::from_slice(&fs::read(&fixture.graph).assert_value()).assert_value();
    graph.as_object_mut().assert_value().remove("networks");
    write_json(&fixture.graph, &graph);
    let run = fixture.submit_detached("networks").await;
    fixture.wait_succeeded(&run, "networks").await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    for (node, iteration) in [
        ("sender", 1),
        ("receiver", 1),
        ("sender", 2),
        ("receiver", 2),
        ("outsider", 0),
    ] {
        assert_eq!(
            network_capture(&fixture, &run, (node, iteration), "capability"),
            "false"
        );
        let prompt = network_capture(&fixture, &run, (node, iteration), "prompt");
        assert!(!prompt.contains("<steering-networks>"));
        for kind in ["deny-network", "deny-broadcast"] {
            let denied: Value =
                serde_json::from_str(&network_capture(&fixture, &run, (node, iteration), kind))
                    .assert_value();
            assert_ne!(denied["status"], 0, "{denied}");
        }
    }
    assert!(
        fixture.json(&["vectors", &run], "networks").await["vectors"]
            .as_array()
            .assert_value()
            .is_empty()
    );
}

fn network_rollback_fixture() -> LocalFixture {
    let fixture = network_fixture("rollback", false);
    let mut chooser = network_step("chooser");
    chooser["kind"] = json!("verifier");
    chooser["signals"] = json!({"route":["first","next"]});
    chooser["diagnostic"] = json!({"kind":"null"});
    let mut graph: Value =
        serde_json::from_slice(&fs::read(&fixture.graph).assert_value()).assert_value();
    for name in ["alpha", "beta"] {
        graph["networks"][name]["members"] = json!(["sender", "rerouted", "receiver"]);
    }
    graph["root"]["children"] = json!([
        chooser,
        {"kind":"choice","name":"route","state":{"kind":"null"},"promotedStatePaths":[],
         "branches":[{"when":{"kind":"in","value":{"source":"signal","name":"chooser","field":"route"},
                       "labels":["first"]},"node":network_step("sender")}],
         "otherwise":network_step("rerouted")},
        network_step("receiver"),
        {"kind":"fail","name":"retry","reason":"network_retry"}
    ]);
    write_json(&fixture.graph, &graph);
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(&fixture.runtime).assert_value()).assert_value();
    runtime["nodes"]["chooser"] = runtime["nodes"]["sender"].clone();
    runtime["nodes"]["rerouted"] = runtime["nodes"]["sender"].clone();
    runtime["nodes"]
        .as_object_mut()
        .assert_value()
        .remove("outsider");
    runtime["workspaceCheckpoints"] = json!(true);
    write_json(&fixture.runtime, &runtime);
    fixture
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_failed_restart_and_earlier_checkpoint_deliver_to_distinct_new_branch_instances() {
    let fixture = network_rollback_fixture();
    let mut run = fixture.submit_detached("networks").await;
    fixture
        .wait_terminal(&run, "networks", "network_retry")
        .await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    network_no_messages(&fixture, &run, "sender", 0);
    let original = network_received(&fixture, &run, "receiver", 0);
    assert_eq!(original.matches(ONCE).count(), 1);
    let source: Value =
        serde_json::from_str(&network_capture(&fixture, &run, ("sender", 0), "identity"))
            .assert_value();
    let mut highest = 0;
    for node in ["chooser", "sender", "receiver"] {
        let identity: Value =
            serde_json::from_str(&network_capture(&fixture, &run, (node, 0), "identity"))
                .assert_value();
        highest = highest.max(identity["nodeInstance"].as_u64().assert_value());
    }
    for (round, checkpoint) in [(1, false), (2, true)] {
        let checkpoint_id = if checkpoint {
            Some(
                fixture
                    .checkpoints(&run)
                    .await
                    .iter()
                    .find(|checkpoint| checkpoint["node"] == "chooser")
                    .assert_value_with("the successor captured its earlier chooser checkpoint")
                    ["checkpointId"]
                    .as_str()
                    .assert_value()
                    .to_owned(),
            )
        } else {
            None
        };
        let mut args = vec!["resume", &run];
        if let Some(checkpoint_id) = &checkpoint_id {
            args.extend([
                "--from-checkpoint",
                checkpoint_id,
                "--discard-workspace-changes",
            ]);
        }
        let next = fixture.json(&args, "networks").await;
        let next = next["runId"].as_str().assert_value().to_owned();
        fixture
            .wait_terminal(&next, "networks", "network_retry")
            .await;
        wait_for_exit(fixture.ready_pid(&next)).await;
        let previous_highest = highest;
        for node in ["chooser", "rerouted", "receiver"] {
            let identity: Value =
                serde_json::from_str(&network_capture(&fixture, &next, (node, 0), "identity"))
                    .assert_value();
            let instance = identity["nodeInstance"].as_u64().assert_value();
            assert!(
                instance > previous_highest,
                "{node} reused a predecessor identity"
            );
            highest = highest.max(instance);
        }
        for node in ["rerouted", "receiver"] {
            let received = network_received(&fixture, &next, node, 0);
            assert_eq!(
                received.matches(ONCE).count(),
                1,
                "{node} lost once delivery after rollback"
            );
            assert_eq!(received.matches(STANDING).count(), 1);
        }
        let status = fixture.json(&["status", &next], "networks").await;
        let steers = status["steers"].as_array().assert_value();
        assert_eq!(steers.len(), 2, "the alternate branch must not republish");
        assert_eq!(steers[0]["network"]["origin"], source);
        assert_eq!(steers[0]["receivedExecutions"], 1 + 2 * round);
        assert_eq!(steers[1]["receivedExecutions"], 1 + 2 * round);
        run = next;
    }
}

mod authority {
    include!("network_steering_authority.rs");
}
