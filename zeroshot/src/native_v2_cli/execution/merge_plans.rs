use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use openengine_cluster_protocol::{
    IdempotencyKey, MergePlan, MergePlanRunName, MergePlanRunRequest, MergePlanSource,
    MergePlanState, RunProfile, RunProfileName, RunProfileScope, RunProfileSelector, RunTitle,
    MAX_MERGE_PLAN_RUNS, MERGE_PLAN_SCHEMA,
};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration as TimeDuration, OffsetDateTime};

use super::submission::{read_json, select_connections, validate_github_token};
use super::{CliExecutionContext, write_json};
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_cli::{
    CliOutcome, DetachSignal, MergePlanSelector, MergePlanSubmitCommand, NativeV2CliBackend,
    NativeV2CliCommand, NativeV2CliError, PreparedMergePlanRequest,
};

const PLAN_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MergePlanManifest {
    schema: String,
    title: RunTitle,
    source: MergePlanSource,
    profile: String,
    expires_at: String,
    runs: BTreeMap<String, MergePlanManifestRun>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergePlanManifestRun {
    input: serde_json::Value,
    #[serde(default)]
    needs: Vec<String>,
}

struct ValidatedMergePlan {
    title: RunTitle,
    source: MergePlanSource,
    profile: RunProfileSelector,
    expires_at: String,
    runs: Vec<MergePlanRunRequest>,
}

pub(super) fn validate_file(
    path: &Path,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError> {
    load_manifest(path)?;
    write_json(output, &serde_json::json!({ "valid": true }))?;
    Ok(CliOutcome::Completed)
}

pub(super) async fn execute<B, S>(
    command: NativeV2CliCommand,
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
{
    match command {
        NativeV2CliCommand::PlanSubmit(command) => submit(command, context, signal, output).await,
        NativeV2CliCommand::PlanStatus(selector) => {
            unary(selector, context.backend, output, PlanOperation::Status).await
        }
        NativeV2CliCommand::PlanWatch(selector) => {
            watch(selector, context.backend, signal, output).await
        }
        NativeV2CliCommand::PlanForceStop(selector) => {
            unary(selector, context.backend, output, PlanOperation::Force).await
        }
        _ => Err(NativeV2CliError::Usage(
            "expected a merge-plan operation".to_owned(),
        )),
    }
}

async fn submit<B, S>(
    command: MergePlanSubmitCommand,
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
{
    let manifest = load_manifest(&command.file)?;
    let profile = context
        .backend
        .profile_show(Some(&command.target), manifest.profile.clone())
        .await?;
    validate_profile_and_inputs(&profile, &manifest.runs).await?;
    let request = prepared_request(
        command.submission_key,
        manifest,
        &profile,
        context.environment,
    )?;
    let plan = context
        .backend
        .merge_plan_submit(&command.target, request)
        .await?;
    write_json(output, &plan)?;
    if command.detach {
        return Ok(CliOutcome::Detached);
    }
    let outcome = follow(
        plan,
        PlanFollow {
            target: &command.target,
            backend: context.backend,
        },
        signal,
        output,
    )
    .await?;
    foreground_outcome(outcome)
}

fn prepared_request(
    submission_key: IdempotencyKey,
    manifest: ValidatedMergePlan,
    profile: &RunProfile,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<PreparedMergePlanRequest, NativeV2CliError> {
    let connections = select_connections(&profile.runtime, environment)?;
    let github_token = environment("GH_TOKEN")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| NativeV2CliError::GitHubToken)
        })
        .transpose()?
        .map(validate_github_token)
        .transpose()?;
    Ok(PreparedMergePlanRequest {
        submission_key,
        title: manifest.title,
        expires_at: manifest.expires_at,
        source: manifest.source,
        profile: manifest.profile,
        runs: manifest.runs,
        connections,
        github_token,
    })
}

async fn validate_profile_and_inputs(
    profile: &RunProfile,
    runs: &[MergePlanRunRequest],
) -> Result<(), NativeV2CliError> {
    NativeV2Admission
        .validate_merge_profile(&profile.graph, &profile.runtime)
        .await
        .map_err(NativeV2CliError::InvalidRun)?;
    for run in runs {
        profile
            .graph
            .initial_input
            .validate_value(&run.initial_input)
            .map_err(|error| NativeV2CliError::InitialInput(error.to_string()))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PlanOperation {
    Status,
    Force,
}

async fn unary<B: NativeV2CliBackend>(
    selector: MergePlanSelector,
    backend: &B,
    output: &mut impl Write,
    operation: PlanOperation,
) -> Result<CliOutcome, NativeV2CliError> {
    let plan = match operation {
        PlanOperation::Status => {
            backend
                .merge_plan_status(&selector.target, selector.plan_id)
                .await?
        }
        PlanOperation::Force => {
            backend
                .merge_plan_force(&selector.target, selector.plan_id)
                .await?
        }
    };
    write_json(output, &plan)?;
    Ok(CliOutcome::Completed)
}

async fn watch<B, S>(
    selector: MergePlanSelector,
    backend: &B,
    signal: &mut S,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
{
    let plan = backend
        .merge_plan_status(&selector.target, selector.plan_id)
        .await?;
    write_json(output, &plan)?;
    let outcome = follow(
        plan,
        PlanFollow {
            target: &selector.target,
            backend,
        },
        signal,
        output,
    )
    .await?;
    foreground_outcome(outcome)
}

struct PlanFollow<'a, B> {
    target: &'a str,
    backend: &'a B,
}

async fn follow<B, S>(
    mut current: MergePlan,
    follow: PlanFollow<'_, B>,
    signal: &mut S,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
{
    while !current.state.is_terminal() {
        tokio::select! {
            () = signal.wait() => return Ok(CliOutcome::Detached),
            () = tokio::time::sleep(PLAN_POLL_INTERVAL) => {}
        }
        let next = follow
            .backend
            .merge_plan_status(follow.target, current.plan_id.clone())
            .await?;
        if next != current {
            write_json(output, &next)?;
            current = next;
        }
    }
    Ok(outcome_for_plan(&current))
}

fn outcome_for_plan(plan: &MergePlan) -> CliOutcome {
    match plan.state {
        MergePlanState::Succeeded => CliOutcome::Finished,
        MergePlanState::Failed | MergePlanState::Cancelled | MergePlanState::Expired => {
            CliOutcome::Failed
        }
        MergePlanState::Queued | MergePlanState::Running => CliOutcome::Completed,
    }
}

fn foreground_outcome(outcome: CliOutcome) -> Result<CliOutcome, NativeV2CliError> {
    match outcome {
        CliOutcome::Failed => Err(NativeV2CliError::MergePlanFailed),
        outcome => Ok(outcome),
    }
}

fn load_manifest(path: &Path) -> Result<ValidatedMergePlan, NativeV2CliError> {
    let manifest = read_json::<MergePlanManifest>("merge plan", path)?;
    if manifest.schema != MERGE_PLAN_SCHEMA {
        return Err(plan_usage(format!("schema must be {MERGE_PLAN_SCHEMA:?}")));
    }
    validate_deadline(&manifest.expires_at)?;
    let profile = parse_profile(&manifest.profile)?;
    let runs = compile_runs(&manifest.title, manifest.runs)?;
    Ok(ValidatedMergePlan {
        title: manifest.title,
        source: manifest.source,
        profile,
        expires_at: manifest.expires_at,
        runs,
    })
}

fn validate_deadline(value: &str) -> Result<(), NativeV2CliError> {
    validate_deadline_at(value, OffsetDateTime::now_utc())
}

fn validate_deadline_at(value: &str, now: OffsetDateTime) -> Result<(), NativeV2CliError> {
    let expires = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| plan_usage("expiresAt must be an RFC 3339 timestamp"))?;
    if expires <= now {
        return Err(plan_usage("expiresAt must be in the future"));
    }
    if expires > now + TimeDuration::days(7) {
        return Err(plan_usage("expiresAt must be at most seven days away"));
    }
    Ok(())
}

fn parse_profile(value: &str) -> Result<RunProfileSelector, NativeV2CliError> {
    let (scope, name) = value
        .split_once(':')
        .ok_or_else(|| plan_usage("profile must be user:NAME or org:NAME"))?;
    let scope = match scope {
        "user" => RunProfileScope::User,
        "org" => RunProfileScope::Org,
        _ => return Err(plan_usage("profile must be user:NAME or org:NAME")),
    };
    let name = RunProfileName::new(name)
        .map_err(|error| plan_usage(format!("invalid profile: {error}")))?;
    Ok(RunProfileSelector { scope, name })
}

fn compile_runs(
    title: &RunTitle,
    raw: BTreeMap<String, MergePlanManifestRun>,
) -> Result<Vec<MergePlanRunRequest>, NativeV2CliError> {
    if raw.is_empty() || raw.len() > MAX_MERGE_PLAN_RUNS {
        return Err(plan_usage(format!(
            "runs must contain 1..={MAX_MERGE_PLAN_RUNS} nodes"
        )));
    }
    let names = raw
        .keys()
        .map(|name| {
            MergePlanRunName::new(name)
                .map_err(|error| plan_usage(format!("invalid run name {name:?}: {error}")))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut runs = Vec::with_capacity(raw.len());
    for (raw_name, run) in raw {
        let name =
            MergePlanRunName::new(raw_name).map_err(|error| plan_usage(error.to_string()))?;
        validate_derived_title(title, &name)?;
        let needs = compile_needs(&name, run.needs, &names)?;
        runs.push(MergePlanRunRequest {
            name,
            needs,
            initial_input: run.input,
        });
    }
    validate_acyclic(&runs)?;
    Ok(runs)
}

fn validate_derived_title(
    title: &RunTitle,
    name: &MergePlanRunName,
) -> Result<(), NativeV2CliError> {
    RunTitle::new(format!("{}: {}", title.as_str(), name.as_str()))
        .map(|_| ())
        .map_err(|_| {
            plan_usage(format!(
                "derived title for run {:?} is too long",
                name.as_str()
            ))
        })
}

fn compile_needs(
    name: &MergePlanRunName,
    raw: Vec<String>,
    names: &BTreeSet<MergePlanRunName>,
) -> Result<Vec<MergePlanRunName>, NativeV2CliError> {
    let needs = raw
        .into_iter()
        .map(|need| {
            MergePlanRunName::new(need)
                .map_err(|error| plan_usage(format!("invalid dependency: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let unique = needs.iter().cloned().collect::<BTreeSet<_>>();
    if unique.len() != needs.len() {
        return Err(plan_usage(format!(
            "run {:?} lists a dependency more than once",
            name.as_str()
        )));
    }
    if unique.contains(name) {
        return Err(plan_usage(format!(
            "run {:?} cannot depend on itself",
            name.as_str()
        )));
    }
    if let Some(missing) = unique.iter().find(|need| !names.contains(*need)) {
        return Err(plan_usage(format!(
            "run {:?} needs missing run {:?}",
            name.as_str(),
            missing.as_str()
        )));
    }
    Ok(unique.into_iter().collect())
}

fn validate_acyclic(runs: &[MergePlanRunRequest]) -> Result<(), NativeV2CliError> {
    let mut completed = BTreeSet::new();
    loop {
        let before = completed.len();
        for run in runs {
            if run.needs.iter().all(|need| completed.contains(need)) {
                completed.insert(run.name.clone());
            }
        }
        if completed.len() == runs.len() {
            return Ok(());
        }
        if completed.len() == before {
            return Err(plan_usage("runs must form an acyclic graph"));
        }
    }
}

fn plan_usage(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Usage(format!("invalid merge plan: {}", message.into()))
}

#[cfg(test)]
fn test_manifest_json() -> serde_json::Value {
    serde_json::json!({
        "schema": MERGE_PLAN_SCHEMA,
        "title": "Release",
        "source": {"repository": "owner/repo", "branch": "main"},
        "profile": "org:software-change",
        "expiresAt": "2026-09-11T00:00:00Z",
        "runs": {"build": {"input": {}}}
    })
}

#[cfg(test)]
fn test_run(input: i64, needs: &[&str]) -> MergePlanManifestRun {
    MergePlanManifestRun {
        input: serde_json::json!({ "value": input }),
        needs: needs.iter().map(|value| (*value).to_owned()).collect(),
    }
}

#[test]
fn manifest_json_rejects_unknown_fields_at_every_level() {
    let mut top_level = test_manifest_json();
    top_level["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<MergePlanManifest>(top_level).is_err());

    let mut node = test_manifest_json();
    node["runs"]["build"]["outputFrom"] = serde_json::json!("other");
    assert!(serde_json::from_value::<MergePlanManifest>(node).is_err());
}

#[test]
fn manifest_compiler_preserves_static_inputs_and_dependencies() {
    let mut raw = BTreeMap::new();
    raw.insert("build".to_owned(), test_run(1, &[]));
    raw.insert("integrate".to_owned(), test_run(2, &["build"]));
    let runs = compile_runs(&RunTitle::new("Release").unwrap(), raw).unwrap();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].name.as_str(), "build");
    assert!(runs[0].needs.is_empty());
    assert_eq!(runs[1].name.as_str(), "integrate");
    assert_eq!(runs[1].needs[0].as_str(), "build");
    assert_eq!(runs[1].initial_input, serde_json::json!({ "value": 2 }));
}

#[test]
fn manifest_compiler_canonicalizes_dependency_order() {
    let name = MergePlanRunName::new("integrate").unwrap();
    let names = ["backend", "frontend", "integrate"]
        .map(|value| MergePlanRunName::new(value).unwrap())
        .into_iter()
        .collect();
    let needs = compile_needs(
        &name,
        vec!["frontend".to_owned(), "backend".to_owned()],
        &names,
    )
    .unwrap();

    assert_eq!(
        needs
            .iter()
            .map(MergePlanRunName::as_str)
            .collect::<Vec<_>>(),
        ["backend", "frontend"]
    );
}

#[test]
fn manifest_compiler_rejects_invalid_dependency_graphs() {
    let title = RunTitle::new("Release").unwrap();
    let mut missing = BTreeMap::new();
    missing.insert("build".to_owned(), test_run(1, &["absent"]));
    assert!(compile_runs(&title, missing).is_err());

    let mut self_edge = BTreeMap::new();
    self_edge.insert("build".to_owned(), test_run(1, &["build"]));
    assert!(compile_runs(&title, self_edge).is_err());

    let mut cycle = BTreeMap::new();
    cycle.insert("a".to_owned(), test_run(1, &["b"]));
    cycle.insert("b".to_owned(), test_run(2, &["a"]));
    assert!(compile_runs(&title, cycle).is_err());
}

#[test]
fn manifest_requires_scoped_profile_and_rfc3339_deadline() {
    assert!(parse_profile("org:software-change").is_ok());
    assert!(parse_profile("software-change").is_err());
    assert!(parse_profile("local:software-change").is_err());
    let now = OffsetDateTime::parse("2026-09-10T00:00:00Z", &Rfc3339).unwrap();
    assert!(validate_deadline_at("2026-09-11T00:00:00Z", now).is_ok());
    assert!(validate_deadline_at("2026-09-09T00:00:00Z", now).is_err());
    assert!(validate_deadline_at("2026-09-18T00:00:00Z", now).is_err());
    assert!(validate_deadline_at("tomorrow", now).is_err());
}
