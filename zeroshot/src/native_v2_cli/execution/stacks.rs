use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use openengine_cluster_protocol::{
    IdempotencyKey, MAX_STACK_RUNS, RunProfile, RunProfileName, RunProfileScope,
    RunProfileSelector, RunTitle, STACK_SCHEMA, Stack, StackDelivery, StackRunName,
    StackRunRequest, StackSource, StackState,
};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration as TimeDuration, OffsetDateTime};

use super::submission::{read_json, select_connections, validate_github_token};
use super::{CliExecutionContext, write_json};
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_cli::{
    CliOutcome, DetachSignal, NativeV2CliBackend, NativeV2CliCommand, NativeV2CliError,
    PreparedStackRequest, StackSelector, StackSubmitCommand,
};

const STACK_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StackManifest {
    schema: String,
    title: RunTitle,
    /// Validated after the schema so a missing or misspelled mode names the accepted values.
    #[serde(default)]
    delivery: Option<serde_json::Value>,
    source: StackSource,
    profile: String,
    expires_at: String,
    runs: BTreeMap<String, StackManifestRun>,
    #[serde(default)]
    environment: Option<openengine_cluster_protocol::RuntimeEnvironment>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StackManifestRun {
    input: serde_json::Value,
    #[serde(default)]
    needs: Vec<String>,
}

struct ValidatedStack {
    title: RunTitle,
    delivery: StackDelivery,
    source: StackSource,
    profile: RunProfileSelector,
    expires_at: String,
    /// Submission order; stacked pull requests list the linear stack from its base upward.
    runs: Vec<StackRunRequest>,
    environment: Option<openengine_cluster_protocol::RuntimeEnvironment>,
}

pub(super) fn validate_file(
    path: &Path,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError> {
    let stack = load_manifest(path)?;
    let result = match stack.delivery {
        StackDelivery::Merge => serde_json::json!({ "valid": true }),
        StackDelivery::StackedPullRequests => serde_json::json!({
            "valid": true,
            "order": stack.runs.iter().map(|run| run.name.as_str()).collect::<Vec<_>>(),
        }),
    };
    write_json(output, &result)?;
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
        NativeV2CliCommand::StackSubmit(command) => submit(command, context, signal, output).await,
        NativeV2CliCommand::StackStatus(selector) => {
            unary(selector, context.backend, output, StackOperation::Status).await
        }
        NativeV2CliCommand::StackWatch(selector) => {
            watch(selector, context.backend, signal, output).await
        }
        NativeV2CliCommand::StackForceStop(selector) => {
            unary(selector, context.backend, output, StackOperation::Force).await
        }
        _ => Err(NativeV2CliError::Usage(
            "expected a stack operation".to_owned(),
        )),
    }
}

async fn submit<B, S>(
    command: StackSubmitCommand,
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
    validate_profile_and_inputs(&profile, manifest.delivery, &manifest.runs).await?;
    let request = prepared_request(
        command.submission_key,
        manifest,
        &profile,
        context.environment,
    )?;
    let stack = context
        .backend
        .stack_submit(&command.target, request)
        .await?;
    write_json(output, &stack)?;
    if command.detach {
        return Ok(CliOutcome::Detached);
    }
    let outcome = follow(
        stack,
        StackFollow {
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
    manifest: ValidatedStack,
    profile: &RunProfile,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<PreparedStackRequest, NativeV2CliError> {
    crate::native_v2_admission::validate_run_environment(
        &profile.runtime,
        manifest.environment.as_ref(),
    )
    .map_err(NativeV2CliError::InvalidRun)?;
    let connections =
        select_connections(&profile.runtime, manifest.environment.as_ref(), environment)?;
    let github_token = environment("GH_TOKEN")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| NativeV2CliError::GitHubToken)
        })
        .transpose()?
        .map(validate_github_token)
        .transpose()?;
    Ok(PreparedStackRequest {
        submission_key,
        title: manifest.title,
        expires_at: manifest.expires_at,
        delivery: manifest.delivery,
        source: manifest.source,
        profile: manifest.profile,
        runs: manifest.runs,
        environment: manifest.environment,
        connections,
        github_token,
    })
}

async fn validate_profile_and_inputs(
    profile: &RunProfile,
    delivery: StackDelivery,
    runs: &[StackRunRequest],
) -> Result<(), NativeV2CliError> {
    NativeV2Admission
        .validate_stack_profile(&profile.graph, &profile.runtime, delivery)
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
enum StackOperation {
    Status,
    Force,
}

async fn unary<B: NativeV2CliBackend>(
    selector: StackSelector,
    backend: &B,
    output: &mut impl Write,
    operation: StackOperation,
) -> Result<CliOutcome, NativeV2CliError> {
    let stack = match operation {
        StackOperation::Status => {
            backend
                .stack_status(&selector.target, selector.stack_id)
                .await?
        }
        StackOperation::Force => {
            backend
                .stack_force(&selector.target, selector.stack_id)
                .await?
        }
    };
    write_json(output, &stack)?;
    Ok(CliOutcome::Completed)
}

async fn watch<B, S>(
    selector: StackSelector,
    backend: &B,
    signal: &mut S,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
{
    let stack = backend
        .stack_status(&selector.target, selector.stack_id)
        .await?;
    write_json(output, &stack)?;
    let outcome = follow(
        stack,
        StackFollow {
            target: &selector.target,
            backend,
        },
        signal,
        output,
    )
    .await?;
    foreground_outcome(outcome)
}

struct StackFollow<'a, B> {
    target: &'a str,
    backend: &'a B,
}

async fn follow<B, S>(
    mut current: Stack,
    follow: StackFollow<'_, B>,
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
            () = tokio::time::sleep(STACK_POLL_INTERVAL) => {}
        }
        let next = follow
            .backend
            .stack_status(follow.target, current.stack_id.clone())
            .await?;
        if next != current {
            write_json(output, &next)?;
            current = next;
        }
    }
    Ok(outcome_for_stack(&current))
}

fn outcome_for_stack(stack: &Stack) -> CliOutcome {
    match stack.state {
        StackState::Succeeded => CliOutcome::Finished,
        StackState::Failed | StackState::Cancelled | StackState::Expired => CliOutcome::Failed,
        StackState::Queued | StackState::Running => CliOutcome::Completed,
    }
}

fn foreground_outcome(outcome: CliOutcome) -> Result<CliOutcome, NativeV2CliError> {
    match outcome {
        CliOutcome::Failed => Err(NativeV2CliError::StackFailed),
        outcome => Ok(outcome),
    }
}

fn load_manifest(path: &Path) -> Result<ValidatedStack, NativeV2CliError> {
    let manifest = read_json::<StackManifest>("stack", path)?;
    if manifest.schema != STACK_SCHEMA {
        return Err(stack_usage(format!("schema must be {STACK_SCHEMA:?}")));
    }
    let delivery = parse_delivery(manifest.delivery)?;
    if let Some(environment) = &manifest.environment {
        environment
            .validate()
            .map_err(|error| stack_usage(error.to_string()))?;
    }
    validate_deadline(&manifest.expires_at)?;
    let profile = parse_profile(&manifest.profile)?;
    let runs = compile_runs(&manifest.title, manifest.runs)?;
    let order = dependency_order(&runs)?;
    let runs = match delivery {
        StackDelivery::Merge => runs,
        StackDelivery::StackedPullRequests => linearize(runs, &order),
    };
    Ok(ValidatedStack {
        title: manifest.title,
        delivery,
        source: manifest.source,
        profile,
        expires_at: manifest.expires_at,
        environment: manifest.environment,
        runs,
    })
}

fn parse_delivery(value: Option<serde_json::Value>) -> Result<StackDelivery, NativeV2CliError> {
    value
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            stack_usage(format!(
                "delivery must be {:?} or {:?}",
                StackDelivery::Merge.as_str(),
                StackDelivery::StackedPullRequests.as_str()
            ))
        })
}

fn validate_deadline(value: &str) -> Result<(), NativeV2CliError> {
    validate_deadline_at(value, OffsetDateTime::now_utc())
}

fn validate_deadline_at(value: &str, now: OffsetDateTime) -> Result<(), NativeV2CliError> {
    let expires = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| stack_usage("expiresAt must be an RFC 3339 timestamp"))?;
    if expires <= now {
        return Err(stack_usage("expiresAt must be in the future"));
    }
    if expires > now + TimeDuration::days(7) {
        return Err(stack_usage("expiresAt must be at most seven days away"));
    }
    Ok(())
}

fn parse_profile(value: &str) -> Result<RunProfileSelector, NativeV2CliError> {
    let (scope, name) = value
        .split_once(':')
        .ok_or_else(|| stack_usage("profile must be user:NAME or org:NAME"))?;
    let scope = match scope {
        "user" => RunProfileScope::User,
        "org" => RunProfileScope::Org,
        _ => return Err(stack_usage("profile must be user:NAME or org:NAME")),
    };
    let name = RunProfileName::new(name)
        .map_err(|error| stack_usage(format!("invalid profile: {error}")))?;
    Ok(RunProfileSelector { scope, name })
}

fn compile_runs(
    title: &RunTitle,
    raw: BTreeMap<String, StackManifestRun>,
) -> Result<Vec<StackRunRequest>, NativeV2CliError> {
    if raw.is_empty() || raw.len() > MAX_STACK_RUNS {
        return Err(stack_usage(format!(
            "runs must contain 1..={MAX_STACK_RUNS} nodes"
        )));
    }
    let names = raw
        .keys()
        .map(|name| {
            StackRunName::new(name)
                .map_err(|error| stack_usage(format!("invalid run name {name:?}: {error}")))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut runs = Vec::with_capacity(raw.len());
    for (raw_name, run) in raw {
        let name = StackRunName::new(raw_name).map_err(|error| stack_usage(error.to_string()))?;
        validate_derived_title(title, &name)?;
        let needs = compile_needs(&name, run.needs, &names)?;
        runs.push(StackRunRequest {
            name,
            needs,
            initial_input: run.input,
        });
    }
    Ok(runs)
}

fn validate_derived_title(title: &RunTitle, name: &StackRunName) -> Result<(), NativeV2CliError> {
    RunTitle::new(format!("{}: {}", title.as_str(), name.as_str()))
        .map(|_| ())
        .map_err(|_| {
            stack_usage(format!(
                "derived title for run {:?} is too long",
                name.as_str()
            ))
        })
}

fn compile_needs(
    name: &StackRunName,
    raw: Vec<String>,
    names: &BTreeSet<StackRunName>,
) -> Result<Vec<StackRunName>, NativeV2CliError> {
    let needs = raw
        .into_iter()
        .map(|need| {
            StackRunName::new(need)
                .map_err(|error| stack_usage(format!("invalid dependency: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let unique = needs.iter().cloned().collect::<BTreeSet<_>>();
    if unique.len() != needs.len() {
        return Err(stack_usage(format!(
            "run {:?} lists a dependency more than once",
            name.as_str()
        )));
    }
    if unique.contains(name) {
        return Err(stack_usage(format!(
            "run {:?} cannot depend on itself",
            name.as_str()
        )));
    }
    if let Some(missing) = unique.iter().find(|need| !names.contains(*need)) {
        return Err(stack_usage(format!(
            "run {:?} needs missing run {:?}",
            name.as_str(),
            missing.as_str()
        )));
    }
    Ok(unique.into_iter().collect())
}

/// Orders runs after everything they need, taking the first ready run by name at each step.
fn dependency_order(runs: &[StackRunRequest]) -> Result<Vec<StackRunName>, NativeV2CliError> {
    let mut order = Vec::with_capacity(runs.len());
    let mut placed = BTreeSet::new();
    while order.len() < runs.len() {
        let next = runs
            .iter()
            .filter(|run| {
                !placed.contains(&run.name) && run.needs.iter().all(|need| placed.contains(need))
            })
            .map(|run| &run.name)
            .min()
            .cloned()
            .ok_or_else(|| stack_usage("runs must form an acyclic graph"))?;
        placed.insert(next.clone());
        order.push(next);
    }
    Ok(order)
}

/// Replaces each run's needs with its predecessor in `order`, the only stack a PR chain allows.
fn linearize(runs: Vec<StackRunRequest>, order: &[StackRunName]) -> Vec<StackRunRequest> {
    let mut by_name = runs
        .into_iter()
        .map(|run| (run.name.clone(), run))
        .collect::<BTreeMap<_, _>>();
    let mut previous = None;
    order
        .iter()
        .filter_map(|name| by_name.remove(name))
        .map(|mut run| {
            run.needs = previous.replace(run.name.clone()).into_iter().collect();
            run
        })
        .collect()
}

fn stack_usage(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Usage(format!("invalid stack: {}", message.into()))
}

#[cfg(test)]
#[path = "stacks/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stacks/environment_tests.rs"]
mod environment_tests;
