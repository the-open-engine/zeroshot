//! Documented commands are an interface. People and their agents copy the README, the docs site
//! and `docs/llms.txt`, so every `zeroshot run` there must parse, and the first-run runtimes must
//! pass the same admission `--validate-only` runs. Text-only rules, which also run on docs-only
//! changes, live in `tests/tooling/docs-agent-contract.test.js`.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::RuntimeLane;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::Value;

use super::*;

/// The generated CLI reference shows usage text, not examples.
const GENERATED: [&str; 2] = ["docs/zeroshot-cli.md", "docs/zeroshot-cli.html"];

struct DocumentedRun {
    location: String,
    words: Vec<String>,
}

impl DocumentedRun {
    fn has(&self, flag: &str) -> bool {
        self.words
            .iter()
            .any(|word| word == flag || word.starts_with(&format!("{flag}=")))
    }

    fn arguments(&self) -> Vec<OsString> {
        self.words.iter().skip(1).map(OsString::from).collect()
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .assert_value()
        .to_path_buf()
}

fn documentation_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).assert_value() {
        let path = entry.assert_value().path();
        if path.is_dir() {
            documentation_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "md" || extension == "txt")
        {
            files.push(path);
        }
    }
}

fn documented_sources() -> Vec<(String, String)> {
    let root = repository_root();
    let mut files = vec![root.join("README.md")];
    documentation_files(&root.join("docs"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .assert_value()
                .to_string_lossy()
                .replace('\\', "/");
            (relative, std::fs::read_to_string(&path).assert_value())
        })
        .filter(|(relative, _)| !GENERATED.contains(&relative.as_str()))
        .collect()
}

/// Splits one shell command line the way a POSIX shell would for the quoting used in the docs.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    for character in command.chars() {
        match (quote, character) {
            (Some(open), _) if character == open => quote = None,
            (Some(_), _) => current.push(character),
            (None, '\'' | '"') => {
                quote = Some(character);
                started = true;
            }
            (None, '#') if !started => break,
            (None, _) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, _) => {
                current.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

/// Every `zeroshot run` inside a fenced block, with backslash continuations joined.
fn documented_runs_in(file: &str, text: &str) -> Vec<DocumentedRun> {
    let mut runs = Vec::new();
    let mut fenced = false;
    let mut lines = text.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let command = trimmed.strip_prefix("$ ").unwrap_or(trimmed);
        if !fenced || !command.starts_with("zeroshot run ") {
            continue;
        }
        let mut joined = command.to_owned();
        while joined.ends_with('\\') {
            joined.pop();
            let Some((_, next)) = lines.next() else { break };
            joined.push(' ');
            joined.push_str(next.trim());
        }
        runs.push(DocumentedRun {
            location: format!("{file}:{}", index + 1),
            words: shell_words(&joined),
        });
    }
    runs
}

fn documented_runs() -> Vec<DocumentedRun> {
    documented_sources()
        .iter()
        .flat_map(|(file, text)| documented_runs_in(file, text))
        .collect()
}

/// JSON examples in fenced blocks titled `name` or `name for ...`, keyed by title.
fn titled_json(text: &str, name: &str) -> Vec<(String, Value)> {
    let mut examples = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(title) = line
            .trim()
            .strip_prefix("```json title=\"")
            .and_then(|rest| rest.strip_suffix('"'))
        else {
            continue;
        };
        if title != name && !title.starts_with(&format!("{name} for ")) {
            continue;
        }
        let body = lines
            .by_ref()
            .take_while(|line| !line.trim().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n");
        examples.push((title.to_owned(), serde_json::from_str(&body).assert_value()));
    }
    examples
}

/// Every harness needs a first-run example. The match is exhaustive, so adding a harness stops
/// compiling here until its example is documented and listed.
const fn documented_harness(lane: RuntimeLane) -> &'static str {
    match lane {
        RuntimeLane::Codex { .. } => "codex",
        RuntimeLane::Claude { .. } => "claude",
        RuntimeLane::Copilot { .. } => "copilot",
        RuntimeLane::Pi { .. } => "pi",
    }
}
const HARNESSES: [&str; 4] = ["codex", "claude", "copilot", "pi"];

#[test]
fn shell_words_follow_the_documented_quoting() {
    assert_eq!(
        shell_words(r#"zeroshot run --title "Add JSON output" --input 'in put.json' # note"#),
        [
            "zeroshot",
            "run",
            "--title",
            "Add JSON output",
            "--input",
            "in put.json"
        ]
    );
    let runs = documented_runs_in(
        "example.md",
        "```console\n$ zeroshot run \\\n  --title \"T\" \\\n  --pr\n```\nzeroshot run --outside\n",
    );
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].location, "example.md:2");
    assert_eq!(runs[0].words, ["zeroshot", "run", "--title", "T", "--pr"]);
}

#[test]
fn every_documented_run_command_parses() {
    let runs = documented_runs();
    for source in ["README.md:", "docs/llms.txt:"] {
        assert!(
            runs.iter().any(|run| run.location.starts_with(source)),
            "no `zeroshot run` example found in {source}"
        );
    }
    let rejected = runs
        .iter()
        .filter_map(|run| {
            parse_native_v2_args(run.arguments())
                .err()
                .map(|error| format!("{}: {error}", run.location))
        })
        .collect::<Vec<_>>();
    assert!(
        rejected.is_empty(),
        "documented commands the CLI rejects:\n{}",
        rejected.join("\n")
    );
}

#[tokio::test]
async fn first_run_runtimes_validate_for_every_harness() {
    let file = "docs/getting-started/first-run.md";
    let (_, text) = documented_sources()
        .into_iter()
        .find(|(relative, _)| relative == file)
        .assert_value();
    let validation = documented_runs_in(file, &text)
        .into_iter()
        .find(|run| run.has("--validate-only"))
        .assert_value();
    let [(_, input)] = titled_json(&text, "input.json").try_into().assert_value();
    let runtimes = titled_json(&text, "runtime.json");

    let workspace = tempfile::tempdir().assert_value();
    let files = tempfile::tempdir().assert_value();
    let input_path = files.path().join("input.json");
    let runtime_path = files.path().join("runtime.json");
    std::fs::write(&input_path, serde_json::to_vec(&input).assert_value()).assert_value();

    let mut harnesses = BTreeSet::new();
    for (title, runtime) in runtimes {
        let lane: RuntimeLane = serde_json::from_value(serde_json::json!({
            "harness": runtime["harness"],
            "provider": runtime["provider"],
        }))
        .unwrap_or_else(|error| panic!("{file} `{title}` names an unknown lane: {error}"));
        assert!(
            harnesses.insert(documented_harness(lane)),
            "{file} documents {} twice",
            lane.harness_name()
        );
        std::fs::write(&runtime_path, serde_json::to_vec(&runtime).assert_value()).assert_value();

        let mut arguments = Vec::new();
        let mut words = validation.arguments().into_iter();
        while let Some(word) = words.next() {
            let replacement = match word.to_str() {
                Some("--input") => Some(&input_path),
                Some("--uniform-runtime-config") => Some(&runtime_path),
                _ => None,
            };
            arguments.push(word);
            if let Some(path) = replacement {
                words.next();
                arguments.push(path.as_os_str().to_owned());
            }
        }
        arguments.extend([
            OsString::from("--cwd"),
            workspace.path().as_os_str().to_owned(),
        ]);

        let backend = FakeBackend::default();
        let mut output = Vec::new();
        let outcome = execute_native_v2_cli(
            parse_native_v2_args(arguments).assert_value(),
            &backend,
            &mut NeverDetach,
            &mut output,
        )
        .await
        .unwrap_or_else(|error| panic!("{file} `{title}` fails --validate-only: {error}"));
        assert_eq!(outcome, CliOutcome::Completed, "{file} `{title}`");
        assert_eq!(output, b"{\"valid\":true}\n", "{file} `{title}`");
        assert!(
            backend.calls().is_empty(),
            "{file} `{title}` contacted a target"
        );
    }
    assert_eq!(
        harnesses,
        BTreeSet::from(HARNESSES),
        "{file} needs one runtime.json example per harness"
    );
}
