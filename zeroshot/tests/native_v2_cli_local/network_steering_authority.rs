use super::*;

const MANDATE_DENIAL: &str = "node messages require an authorized --network";

fn authority_fixture() -> LocalFixture {
    let fixture = network_fixture("authority", false);
    let mut graph: Value =
        serde_json::from_slice(&fs::read(&fixture.graph).assert_value()).assert_value();
    let mut sender = network_step("sender");
    let mut receiver = network_step("receiver");
    sender["timeoutMs"] = json!(180000);
    receiver["timeoutMs"] = json!(180000);
    graph["root"]["children"] = json!([
        sender, receiver, network_step("after"),
        {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
    ]);
    let mut runtime: Value =
        serde_json::from_slice(&fs::read(&fixture.runtime).assert_value()).assert_value();
    runtime["nodes"]
        .as_object_mut()
        .assert_value()
        .remove("outsider");
    runtime["nodes"]["after"] = runtime["nodes"]["receiver"].clone();
    runtime["workspaceCheckpoints"] = json!(true);
    write_json(&fixture.graph, &graph);
    write_json(&fixture.runtime, &runtime);
    openengine_cluster_testkit::fixture::write_executable(
        &fixture.root.path("network-provider.py"),
        openengine_cluster_testkit::fixture::python3_script(AUTHORITY_PROVIDER).as_bytes(),
        0o755,
    )
    .assert_value();
    fixture
}

const AUTHORITY_PROVIDER: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, socket, subprocess, sys, time
root = pathlib.Path.cwd()
run, node, instance = [os.environ[k] for k in
    ('ZEROSHOT_RUN_ID', 'ZEROSHOT_NODE_NAME', 'ZEROSHOT_NODE_INSTANCE_ID')]
identity = run+'-'+node+'-0-'+instance
cli = os.environ['NETWORK_CLI']

def save(kind, value):
    path = root/('network-'+identity+'-'+kind)
    temporary = path.with_name(path.name+'.pending')
    temporary.write_text(value if isinstance(value, str) else json.dumps(value))
    temporary.replace(path)

def wait(predicate):
    deadline = time.monotonic()+150
    while not predicate():
        if time.monotonic() >= deadline: raise RuntimeError('authority fixture barrier timed out')
        time.sleep(.01)

save('prompt', sys.stdin.read())
save('capability', bool(os.environ.get('ZEROSHOT_STEERING_CAPABILITY')))
if node == 'sender':
    result = subprocess.run([cli, 'steer', run, '--network', 'alpha', 'authorized-network-marker'],
        text=True, capture_output=True, timeout=10)
    save('publication', {'status':result.returncode, 'stdout':result.stdout, 'stderr':result.stderr})
    if result.returncode: raise RuntimeError('authorized network publication failed: '+result.stderr)
elif node == 'receiver':
    for name in list(os.environ):
        if name.startswith('ZEROSHOT_'): del os.environ[name]
    save('remaining-labels', [name for name in os.environ if name.startswith('ZEROSHOT_')])
    (root/'authority-held').touch()
    jobs_path = root/'authority-jobs.json'
    wait(lambda: jobs_path.exists() or (root/'authority-release').exists())
    if jobs_path.exists():
        def attempt(job):
            target = job['target']
            if job['transport'] == 'cli':
                environment = os.environ.copy()
                environment['ZEROSHOT_STATE_DIR'] = target['state']
                result = subprocess.run([cli, 'steer', target['run'], 'forbidden-'+job['name']],
                    env=environment, text=True, capture_output=True, timeout=10)
                return {'status':result.returncode, 'stdout':result.stdout, 'stderr':result.stderr}
            request = {'jsonrpc':'2.0', 'id':1, 'method':'run/steer', 'params':{
                'runId':target['run'], 'text':'forbidden-'+job['name'], 'mode':'standing'}}
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(10)
                connection.connect(target['socket'])
                connection.sendall((json.dumps(request)+'\n').encode())
                with connection.makefile('rb') as response:
                    return json.loads(response.readline(256*1024))
        for job in json.loads(jobs_path.read_text()):
            if job.get('detached'):
                child = os.fork()
                if child == 0:
                    os.setsid()
                    if os.fork() != 0: os._exit(0)
                    save(job['name'], attempt(job))
                    os._exit(0)
                os.waitpid(child, 0)
                wait(lambda: (root/('network-'+identity+'-'+job['name'])).exists())
            else:
                save(job['name'], attempt(job))
        (root/'authority-checked').touch()
        wait(lambda: (root/'authority-release').exists())
print(json.dumps({'type':'system','subtype':'init','session_id':'authority-'+identity}))
print(json.dumps({'type':'result','subtype':'success','is_error':False,
    'session_id':'authority-'+identity,'structured_output':{'response':None},
    'model_usage_since_last_result':{'fixture-model':{
        'inputTokens':1,'outputTokens':1,'cacheReadInputTokens':0,'cacheCreationInputTokens':0}}}))
"#;

fn authority_target(fixture: &LocalFixture, run: &str, active: bool) -> Value {
    let paths = zeroshot_engine::native_v2_portable_controller::PortableControllerPaths::new(
        fixture.run_storage(run),
    );
    json!({"run":run,"state":fixture.state,"socket":paths.socket(),"active":active})
}

fn authority_jobs(targets: &[Value]) -> Vec<Value> {
    let mut jobs = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        for transport in ["cli", "rpc"] {
            if transport == "rpc" && target["active"] != true {
                continue;
            }
            jobs.push(json!({"name":format!("target-{index}-{transport}"),
                "transport":transport,"target":target}));
        }
    }
    #[cfg(target_os = "linux")]
    for transport in ["cli", "rpc"] {
        jobs.push(json!({"name":format!("detached-{transport}"),
            "transport":transport,"target":targets[0],"detached":true}));
    }
    jobs
}

fn assert_authority_denials(fixture: &LocalFixture, run: &str, jobs: &[Value]) {
    assert_eq!(
        network_capture(fixture, run, ("receiver", 0), "remaining-labels"),
        "[]"
    );
    assert_eq!(
        network_capture(fixture, run, ("receiver", 0), "capability"),
        "false"
    );
    for job in jobs {
        let reply: Value = serde_json::from_str(&network_capture(
            fixture,
            run,
            ("receiver", 0),
            job["name"].as_str().assert_value(),
        ))
        .assert_value();
        if job["transport"] == "cli" {
            assert_ne!(reply["status"], 0, "{job}: {reply}");
            assert!(
                reply["stderr"]
                    .as_str()
                    .assert_value()
                    .contains(MANDATE_DENIAL),
                "{job}: {reply}"
            );
        } else {
            assert_eq!(reply["error"]["message"], MANDATE_DENIAL, "{job}: {reply}");
        }
    }
}

async fn assert_authority_steers(fixture: &LocalFixture, run: &str, expected: usize) {
    let status = fixture.json(&["status", run], "networks").await;
    let steers = status["steers"].as_array().assert_value();
    assert_eq!(
        steers.len(),
        expected,
        "an agent bypass appended a steer: {status}"
    );
    assert!(
        steers.iter().all(|steer| {
            !steer["excerpt"]
                .as_str()
                .assert_value()
                .contains("forbidden-")
        }),
        "{status}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn networks_cleared_agent_labels_cannot_use_human_cli_or_rpc_across_runs() {
    let suspended = authority_fixture();
    let suspended_run = suspended.submit_detached("networks").await;
    network_wait(&suspended, "authority-held").await;
    suspended
        .json(&["suspend", &suspended_run], "networks")
        .await;
    fs::write(suspended.repository.join("authority-release"), "go").assert_value();
    suspended.wait_suspended(&suspended_run).await;
    wait_for_exit(suspended.ready_pid(&suspended_run)).await;
    let active = authority_fixture();
    let active_run = active.submit_detached("networks").await;
    network_wait(&active, "authority-held").await;
    let source = authority_fixture();
    let source_run = source.submit_detached("networks").await;
    network_wait(&source, "authority-held").await;
    for (fixture, run) in [(&source, &source_run), (&suspended, &suspended_run)] {
        let owner = fixture
            .json(&["steer", run, "authorized-owner-marker"], "networks")
            .await;
        assert_eq!(owner["steer"]["excerpt"], "authorized-owner-marker");
        assert!(owner["steer"]["network"].is_null());
    }
    let publication: Value = serde_json::from_str(&network_capture(
        &source,
        &source_run,
        ("sender", 0),
        "publication",
    ))
    .assert_value();
    assert_eq!(publication["status"], 0, "{publication}");
    let peer: Value =
        serde_json::from_str(publication["stdout"].as_str().assert_value()).assert_value();
    assert_eq!(peer["steer"]["network"]["origin"]["node"], "sender");
    let jobs = authority_jobs(&[
        authority_target(&source, &source_run, true),
        authority_target(&active, &active_run, true),
        authority_target(&suspended, &suspended_run, false),
    ]);
    write_json(&source.repository.join("authority-jobs.json"), &json!(jobs));
    network_wait(&source, "authority-checked").await;
    assert_authority_denials(&source, &source_run, &jobs);
    for (fixture, run, count) in [
        (&source, &source_run, 2),
        (&active, &active_run, 1),
        (&suspended, &suspended_run, 2),
    ] {
        assert_authority_steers(fixture, run, count).await;
    }
    for (fixture, run) in [(&source, &source_run), (&active, &active_run)] {
        fs::write(fixture.repository.join("authority-release"), "go").assert_value();
        fixture.wait_succeeded(run, "networks").await;
        wait_for_exit(fixture.ready_pid(run)).await;
    }
}
