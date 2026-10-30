//! Optional public native-resource receipts supplement the shared live harness contract.
use std::path::{Path, PathBuf};

use openengine_cluster_testkit::assertions::AssertValue;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub(super) struct NativeProof {
    schema: String,
    run: String,
    workspace: PathBuf,
    receipts: PathBuf,
    harness: Option<String>,
    tool: String,
    instruction: String,
}

impl NativeProof {
    pub(super) fn read(workspace: &Path) -> Option<Self> {
        let path = std::env::var_os("ZEROSHOT_LIVE_NATIVE_PROOF_MANIFEST")?;
        let source = std::fs::read(path).assert_value_with("native proof manifest");
        let proof: Self = serde_json::from_slice(&source).assert_value();
        assert_eq!(proof.schema, "zeroshot.native-parity-fixture/v1");
        assert!(matches!(
            proof.harness.as_deref(),
            None | Some("opencode" | "hermes")
        ));
        assert_eq!(proof.workspace.canonicalize().assert_value(), workspace);
        Some(proof)
    }

    pub(super) fn append_instruction(&self, instruction: &mut String, phase: &str) {
        instruction.push_str("\n\n");
        instruction.push_str(&self.instruction);
        instruction.push_str(&format!(
            " Use exactly `{phase}` as the native MCP record `phase` value."
        ));
    }

    pub(super) fn tool_calls(&self) -> usize {
        self.mcp_events().len()
    }

    pub(super) fn assert_new_call(&self, before: usize, node: &str, marker: &str) {
        let calls = self.mcp_events();
        assert!(
            calls.len() >= before,
            "native proof receipts were truncated"
        );
        assert!(
            calls[before..].iter().enumerate().any(|(index, call)| {
                call["kind"] == "tool-call"
                    && call["phase"] == node
                    && call["marker"] == marker
                    && call["authenticated"] == true
                    && call["nativeResources"] == true
                    && (!self.is_hermes()
                        || (call["nativeTransform"] == true
                            && calls[before..before + index].iter().any(|probe| {
                                probe["kind"] == "probe-call"
                                    && probe["authenticated"] == true
                                    && probe["pid"] == call["pid"]
                            })))
            }),
            "live {node} must make a new authenticated native MCP call with recalled memory and native resources"
        );
    }

    pub(super) fn assert_completed(&self) {
        if self.is_hermes() {
            self.assert_hermes();
            return;
        }
        for scope in ["global", "project"] {
            let events = self.records(&format!("plugin-{scope}.jsonl"));
            assert!(
                events.iter().all(|event| {
                    event["scope"] == scope && event["auto"] == false && event["pure"] == false
                }),
                "native {scope} plugin must retain its scope and authored permissions"
            );
            assert!(events.iter().any(|event| event["kind"] == "init"));
            let configurations: Vec<_> = events
                .iter()
                .filter(|event| event["kind"] == "config")
                .collect();
            assert!(
                !configurations.is_empty(),
                "missing native {scope} configuration"
            );
            for configuration in configurations {
                assert_eq!(configuration["deny"], "deny");
                assert_eq!(
                    configuration["commands"],
                    json!(["native-parity-global", "native-parity-project"])
                );
                assert_eq!(
                    configuration["instructions"],
                    json!(["global-rules.md", "project-rules.md"])
                );
                assert_eq!(configuration["mcpPresent"], true);
            }
            for kind in ["tool-before", "tool-after"] {
                assert!(
                    events
                        .iter()
                        .any(|event| event["kind"] == kind && event["tool"] == self.tool),
                    "native {scope} plugin must observe {kind} for the proof MCP tool"
                );
            }
        }
    }

    fn is_hermes(&self) -> bool {
        self.harness.as_deref() == Some("hermes")
    }

    fn assert_hermes(&self) {
        let events = self.records("plugin-global.jsonl");
        assert!(
            events
                .iter()
                .all(|event| { event["scope"] == "global" && event["harness"] == "hermes" })
        );
        let initializations: Vec<_> = events
            .iter()
            .filter(|event| event["kind"] == "init")
            .collect();
        assert!(!initializations.is_empty(), "missing native Hermes plugin");
        for initialization in initializations {
            assert_eq!(initialization["safeMode"], false);
            assert_eq!(initialization["permissionMode"], "manual");
            assert_eq!(initialization["pluginsEnabled"], true);
            assert_eq!(initialization["mcpPresent"], true);
        }
        assert!(
            events
                .iter()
                .filter(|event| event["kind"] == "tool-before")
                .all(|event| event["auto"] == false)
        );
        for kind in ["tool-before", "tool-after", "tool-transform"] {
            assert!(
                events
                    .iter()
                    .any(|event| { event["kind"] == kind && event["tool"] == self.tool }),
                "native Hermes plugin must observe {kind} for the proof MCP tool"
            );
        }
        assert!(
            events.iter().any(|event| {
                event["kind"] == "tool-transform"
                    && event["tool"] == "mcp__nativeproof__probe"
                    && event["challengeEmitted"] == true
            }),
            "native Hermes result transform must provide the proof challenge"
        );
        assert!(
            !self.receipts.join("disabled-plugin-loaded").exists(),
            "native disabled Hermes plugin must stay disabled"
        );
    }

    fn mcp_events(&self) -> Vec<Value> {
        let events = self.records("mcp.jsonl");
        assert!(
            events
                .iter()
                .all(|event| event["kind"] != "authentication-rejected"),
            "native proof MCP authentication was rejected"
        );
        events
    }

    fn records(&self, name: &str) -> Vec<Value> {
        let source = match std::fs::read_to_string(self.receipts.join(name)) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("native proof {name} could not be read: {error}"),
        };
        source
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let event: Value = serde_json::from_str(line).assert_value();
                assert_eq!(event["schema"], "zeroshot.native-parity/v1");
                assert_eq!(event["run"], self.run);
                event
            })
            .collect()
    }
}
