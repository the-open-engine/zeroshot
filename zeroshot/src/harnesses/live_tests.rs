//! Explicit live contract shared by every registered local harness, including contributions.
//! No credentials, model names or native harness choices are embedded in this test.
#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use openengine_cluster_protocol::{IdempotencyKey, NodeName, RunId, RunSize, RunTitle, WorkerOutcome};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::{HarnessBuildContext, HarnessHost, HarnessRegistry};
use crate::active_memory::hooks::{ActiveMemoryHooks, ActiveMemorySource, HookQuery};
use crate::execution::SessionScope;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_candidate::{NativeV2CandidateConfig, build_local_native_v2_candidate};
use crate::native_v2_candidate::test_support::{
    NodeRequestFixture, TestDirectory, environment_name, full_graph, success_node,
};
use crate::native_v2_capsule::provider_process::{LocalHarnessEnvironment, current_process_environment};
use crate::native_v2_contract::{
    DeclaredConnections, DeclaredEnvironment, NodeRuntimeBinding, RunSubmission, RuntimeLane,
    RuntimePlan,
};
use crate::native_v2_runner::{NodeHandle, NodeRunner};
use crate::worker_catalog::ModelId;

mod test_support;
use test_support::NativeProof;

struct RecalledMemory {
    marker: String,
    queries: Mutex<Vec<(String, String)>>,
    failures: Mutex<Vec<String>>,
}

impl ActiveMemorySource for RecalledMemory {
    fn tool_context(&self, query: &HookQuery) -> String {
        self.queries
            .lock()
            .assert_value()
            .push((query.node_id.clone(), query.session_key.clone()));
        format!(
            "\nActive Memory integration check: include this exact marker in your final response and scratch artifact: {}\n",
            self.marker
        )
    }
    fn graph_context(&self, _node: &str, _session: &str) -> String {
        String::new()
    }
    fn unavailable(&self, reason: &str) {
        self.failures.lock().assert_value().push(reason.to_owned());
    }
}

fn required(name: &str) -> String {
    std::env::var(name).assert_value_with(name)
}

struct LiveSettings {
    lane: RuntimeLane,
    model: String,
    workspace: PathBuf,
    environment: BTreeMap<String, String>,
}

impl LiveSettings {
    fn read() -> Self {
        Self {
            lane: RuntimeLane::new(
                required("ZEROSHOT_LIVE_HARNESS"),
                required("ZEROSHOT_LIVE_PROVIDER"),
            )
            .assert_value(),
            model: required("ZEROSHOT_LIVE_MODEL"),
            workspace: PathBuf::from(required("ZEROSHOT_LIVE_WORKSPACE"))
                .canonicalize()
                .assert_value(),
            environment: current_process_environment(),
        }
    }

    fn binding(&self, registry: &HarnessRegistry, scope: SessionScope) -> NodeRuntimeBinding {
        let access = registry.access(&self.lane).assert_value();
        let available = access.accepted_field_sets.iter().find(|fields| {
            fields
                .iter()
                .all(|name| self.environment.contains_key(*name))
        });
        let mut fields = available
            .into_iter()
            .flat_map(|fields| fields.iter().copied())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        if let Some(extra) = self.environment.get("ZEROSHOT_LIVE_CONNECTION_FIELDS") {
            for name in extra.split(',') {
                assert!(
                    self.environment.contains_key(name),
                    "explicit live connection field {name} is missing"
                );
                fields.insert(name.to_owned());
            }
        }
        let connections = if fields.is_empty() {
            DeclaredConnections::empty()
        } else {
            DeclaredConnections::single(
                "live-provider",
                DeclaredEnvironment::new(fields.iter().map(|name| environment_name(name)))
                    .assert_value(),
            )
            .assert_value()
        };
        NodeRuntimeBinding::Agent {
            lane: None,
            model: ModelId::new(&self.model).assert_value(),
            effort: None,
            context_window: None,
            max_tokens: None,
            session_scope: scope,
            connections,
        }
    }

    fn connection_values(
        &self,
        binding: &NodeRuntimeBinding,
    ) -> BTreeMap<crate::native_v2_contract::EnvironmentVariableName, String> {
        binding
            .declared_connections()
            .environment_names()
            .filter_map(|name| {
                self.environment
                    .get(name.as_str())
                    .map(|value| (name.clone(), value.clone()))
            })
            .collect()
    }

    fn context(&self, registry: &HarnessRegistry, runtime_home: PathBuf) -> HarnessBuildContext {
        crate::execution::platform::private_directory(&runtime_home).assert_value();
        let descriptor = registry.descriptor(&self.lane).assert_value();
        let native = LocalHarnessEnvironment::new(self.environment.clone());
        let executable = crate::execution::platform::find_executable(
            descriptor.executable,
            &self.environment,
            &self.workspace,
        )
        .assert_value_with("live harness executable");
        HarnessBuildContext {
            lane: self.lane.clone(),
            workspace: self.workspace.clone(),
            runtime_home,
            executable,
            prefix_arguments: vec![],
            search_path: self.environment.get("PATH").cloned().unwrap_or_default(),
            base_environment: BTreeMap::new(),
            host: HarnessHost::Local {
                home: native.user_home().map(PathBuf::from),
                environment: native,
            },
        }
    }
}

fn executable(name: &str, instruction: &str, verifier: bool) -> Value {
    let mut node = json!({
        "kind":if verifier {"verifier"} else {"step"}, "name":name,
        "worker":format!("agent.{name}@1"), "instructions":instruction,
        "input":{"kind":"null"}, "output":{"kind":"string"},
        "inputBindings":[], "writeBindings":[], "attempts":1,
    });
    if verifier {
        node["signals"] = json!({});
        node["diagnostic"] = json!({"kind":"null"});
    }
    node
}

async fn completion(handle: &mut NodeHandle) -> crate::native_v2_contract::NodeCompletion {
    match tokio::time::timeout(std::time::Duration::from_secs(300), handle.completion()).await {
        Ok(result) => {
            result.unwrap_or_else(|error| panic!("live harness completion failed: {error}"))
        }
        Err(_) => {
            handle.cancel();
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(30), handle.completion()).await;
            panic!("live harness exceeded the test's five-minute observation budget");
        }
    }
}

#[tokio::test]
#[ignore = "requires explicit live harness/provider/model and a disposable matrix repository"]
async fn registered_harness_live_memory_sessions_and_verifier_contract() {
    let settings = LiveSettings::read();
    let registry = HarnessRegistry::local();
    let native_proof = NativeProof::read(&settings.workspace);
    let directory = TestDirectory::new("live-harness-contract");
    let run = format!("harness-live-{}", uuid::Uuid::now_v7());
    let artifact = format!(".zeroshot-live-{run}.txt");
    let mut worker = format!(
        "Use native tools to inspect README.md and package.json, then execute npm test in this workspace. Write {artifact} containing exactly the marker delivered by Active Memory through tool context. Your final output must contain that marker and the test result. Do not modify tracked source or run Git commands."
    );
    let mut verifier = format!(
        "Independently inspect {artifact} and run npm test using native tools. Your final output must include the exact marker delivered by Active Memory and your verification result. Do not edit files or run Git commands."
    );
    if let Some(proof) = &native_proof {
        proof.append_instruction(&mut worker, "worker");
        proof.append_instruction(&mut verifier, "verifier");
    }
    let bindings = runtime_bindings(&settings, &registry);
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Explicit live harness contract").assert_value(),
            graph: full_graph(vec![
                executable("worker", &worker, false),
                executable("verifier", &verifier, true),
                success_node(),
            ]),
            initial_input: Value::Null,
            runtime: RuntimePlan::new(settings.lane.clone(), RunSize::Medium, bindings.clone()),
            environment: None,
            source: None,
            submission_key: IdempotencyKey::new(&run).assert_value(),
        })
        .await
        .assert_value();
    let memory = Arc::new(RecalledMemory {
        marker: format!("memory-proof-{}", uuid::Uuid::now_v7()),
        queries: Mutex::new(Vec::new()),
        failures: Mutex::new(Vec::new()),
    });
    let hooks = Arc::new(ActiveMemoryHooks::new(memory.clone()).assert_value());
    let runner = build_local_native_v2_candidate(
        &admitted,
        NativeV2CandidateConfig {
            active_memory: Some(hooks),
            lanes: vec![
                registry
                    .configure(settings.context(&registry, directory.child("runtime")))
                    .assert_value(),
            ],
            workspace: settings.workspace.clone(),
            delivery: None,
            comment_delivery: None,
        },
    )
    .assert_value();
    for (node, identity, instruction) in [
        ("worker", (1, 1), &worker),
        ("worker", (1, 2), &worker),
        ("verifier", (2, 3), &verifier),
    ] {
        let binding = bindings[&NodeName::new(node).assert_value()].clone();
        let values = settings.connection_values(&binding);
        let request = NodeRequestFixture {
            run_id: &run,
            node,
            node_instance: identity.0,
            execution: identity.1,
            worker: &format!("agent.{node}@1"),
            instructions: instruction,
            input: Value::Null,
            binding,
            environment: values,
        }
        .into_request();
        let before = memory.queries.lock().assert_value().len();
        let proof_before = native_proof.as_ref().map_or(0, NativeProof::tool_calls);
        let mut handle = runner.start(request).await.assert_value();
        let result = completion(&mut handle).await;
        match result.outcome {
            WorkerOutcome::Verified { output, .. } | WorkerOutcome::Verifier { output, .. } => {
                assert!(output.to_string().contains(&memory.marker))
            }
            outcome => panic!("live {node} returned {outcome:?}"),
        }
        assert!(
            memory.queries.lock().assert_value().len() > before,
            "native tool callback must run in every execution"
        );
        if let Some(proof) = &native_proof {
            proof.assert_new_call(proof_before, node, &memory.marker);
        }
    }
    runner.close_run(&RunId::new(&run)).await;
    if let Some(proof) = &native_proof {
        proof.assert_completed();
    }
    assert_recall(&settings, &memory, &artifact);
}

fn runtime_bindings(
    settings: &LiveSettings,
    registry: &HarnessRegistry,
) -> BTreeMap<NodeName, NodeRuntimeBinding> {
    BTreeMap::from([
        (
            NodeName::new("worker").assert_value(),
            settings.binding(registry, SessionScope::NodeInstance),
        ),
        (
            NodeName::new("verifier").assert_value(),
            settings.binding(registry, SessionScope::Execution),
        ),
    ])
}

fn assert_recall(settings: &LiveSettings, memory: &RecalledMemory, artifact: &str) {
    let failures = memory.failures.lock().assert_value();
    assert!(
        failures.is_empty(),
        "memory evidence unavailable: {failures:?}"
    );
    drop(failures);
    let sessions: BTreeSet<_> = memory
        .queries
        .lock()
        .assert_value()
        .iter()
        .map(|(_, session)| session.clone())
        .collect();
    assert_eq!(
        sessions,
        BTreeSet::from(["node-instance/1".to_owned(), "execution/3".to_owned()])
    );
    let saved = std::fs::read_to_string(settings.workspace.join(artifact)).assert_value();
    assert!(saved.contains(&memory.marker));
    println!(
        "live contract passed: {} / {} — tool recall, reused worker session, fresh verifier, npm test",
        settings.lane, settings.model
    );
}
