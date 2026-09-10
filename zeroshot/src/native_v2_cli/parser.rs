use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

/// Run multi-agent graphs locally or on named Zeroshot targets.
///
/// Single-result commands write JSON. Foreground `run`, `watch`, `logs`, and `attach` stream
/// newline-delimited JSON (NDJSON).
#[derive(Debug, Parser)]
#[command(
    name = "zeroshot",
    version,
    disable_version_flag = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Print the Zeroshot version.
    #[arg(short = 'V', long, exclusive = true)]
    version: bool,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Manage named targets or serve a direct target.
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },

    /// Inspect and manage named runtime connections.
    Connection {
        #[command(subcommand)]
        command: ConnectionCommand,
    },

    /// Manage reusable graph/runtime profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },

    /// Inspect built-in graph templates.
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },

    /// Validate, submit, and observe hosted merge plans.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },

    /// Submit a graph run locally or to a named target.
    ///
    /// When --target is omitted, the run uses the current local repository. A foreground run
    /// follows NDJSON events until completion. --detach returns after submission; Ctrl-C also
    /// detaches from observation without stopping the run. Named-target runs send GH_TOKEN, when
    /// set, for source checkout and Git delivery; providers receive it only when the runtime
    /// declares GH_TOKEN.
    Run(RunArgs),

    #[command(flatten)]
    Utility(UtilityCommand),
}

#[derive(Debug, Subcommand)]
enum UtilityCommand {
    /// List runs as JSON.
    List(TargetRouteArgs),

    /// Read a run's current status as JSON.
    Status(RunSelectorArgs),

    /// Follow a run's durable event stream as NDJSON.
    Watch(RunWatchArgs),

    /// Follow a run's log stream as NDJSON.
    Logs(RunLogsArgs),

    /// Attach to an execution's interactive event stream as NDJSON.
    Attach(AttachArgs),

    /// Force a run to stop and write the result as JSON.
    ForceStop(RunSelectorArgs),

    /// Print the Zeroshot version.
    Version,
}

#[derive(Debug, Subcommand)]
#[command(after_long_help = r#"MANIFEST
The JSON manifest is strict and self-contained:

  {
    "schema": "zeroshot.merge-plan/v1",
    "title": "Release checkout update",
    "source": {"repository": "owner/repo", "branch": "main"},
    "profile": "org:software-change",
    "expiresAt": "<RFC3339 timestamp within 7 days>",
    "runs": {
      "backend": {"input": {"task": "Update the API."}},
      "integrate": {"needs": ["backend"], "input": {"task": "Run release tests."}}
    }
  }

Every run uses the same source and profile. The profile must contain exactly one `builtin.git-delivery.merge@2` node;
pull-request delivery is rejected. `needs` gates readiness but does not pass output between runs.
Cloud assigns every run ID atomically at submission. After a node's dependencies succeed, Cloud
materializes it against an exact source revision. Completion starts a queue window of up to 24
hours, bounded by `expiresAt`.
`expiresAt` must be in the future and no more than
seven days away. Plans cannot be edited or retried in place."#)]
enum PlanCommand {
    /// Validate a merge-plan manifest without contacting a target.
    Validate(PlanFileArgs),

    /// Atomically submit every node in a merge-plan manifest.
    Submit(PlanSubmitArgs),

    /// Read a merge plan's aggregate status as JSON.
    Status(PlanSelectorArgs),

    /// Poll a merge plan and stream changed snapshots as NDJSON.
    Watch(PlanSelectorArgs),

    /// Force every nonterminal run in a merge plan to stop.
    ForceStop(PlanSelectorArgs),
}

#[derive(Debug, Subcommand)]
enum TargetCommand {
    /// Register a named target.
    Add(TargetAddArgs),

    /// Authenticate with a hosted named target.
    ///
    /// On Linux, desktop sessions prefer Secret Service and headless sessions use a durable
    /// private file. Set ZEROSHOT_CREDENTIAL_STORE to auto, system, or file to override
    /// automatic selection.
    Login(TargetNameArgs),

    /// Serve an unauthenticated direct target.
    ///
    /// Direct mode is unauthenticated. Bind or publish it only on trusted networks.
    Serve(TargetServeArgs),
}

#[derive(Debug, Subcommand)]
#[command(after_long_help = r#"CONNECTIONS
A runtime declares a connection key and the exact environment fields it needs. Zeroshot injects
only those fields; secret values never belong in runtime configuration.

`list` returns each key, scope, kind, and field names, never secret values. `set` creates or replaces
a complete static connection, so include every required field. Omit --target for local storage; use
--target NAME for hosted storage. Organization scope requires a hosted target.

Target-managed dynamic kinds are configured through the target rather than `connection set`; `list`
reports each connection's kind.

When a run reports `connection_unavailable`, list connections for the same target and scope, then
set the named key with every required field.

EXAMPLES
  zeroshot connection list
  zeroshot connection list --target prod --scope org
  zeroshot connection set openrouter --field OPENROUTER_API_KEY
  zeroshot connection set openrouter --target prod --field OPENROUTER_API_KEY"#)]
enum ConnectionCommand {
    /// List connection metadata without secret values.
    List(ConnectionRouteArgs),

    /// Create or replace one static connection.
    Set(ConnectionSetArgs),

    /// Delete one connection.
    Delete(ConnectionDeleteArgs),
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ConnectionScopeArg {
    #[default]
    User,
    Org,
}

#[derive(Debug, Args)]
struct ConnectionRouteArgs {
    /// Use this named hosted target. If omitted, use local connections.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,

    /// Select user- or organization-scoped connections.
    #[arg(long, value_enum, default_value_t)]
    scope: ConnectionScopeArg,
}

#[derive(Debug, Args)]
#[command(
    group = ArgGroup::new("connection_input").args(["field", "json_stdin"]).required(true).multiple(false),
    after_long_help = r#"INPUT
Use --field ENV to prompt without echo; repeat it for every required field. Use --json-stdin to read
one non-empty JSON object mapping field names to secret values.

`set` replaces the complete stored static connection for KEY. Existing fields not supplied are
removed. Do not put secret values in shell arguments or runtime configuration."#
)]
struct ConnectionSetArgs {
    /// Unique connection key within the selected scope.
    #[arg(value_name = "KEY")]
    key: String,

    /// Prompt without echo for this environment field. Repeat for multiple fields.
    #[arg(long, value_name = "ENV", action = clap::ArgAction::Append)]
    field: Vec<String>,

    /// Read one JSON object of environment field names to secret values from standard input.
    #[arg(long)]
    json_stdin: bool,

    #[command(flatten)]
    route: ConnectionRouteArgs,
}

#[derive(Debug, Args)]
struct ConnectionDeleteArgs {
    /// Unique connection key within the selected scope.
    #[arg(value_name = "KEY")]
    key: String,

    #[command(flatten)]
    route: ConnectionRouteArgs,
}

#[derive(Debug, Subcommand)]
enum TemplateCommand {
    /// List built-in template names as JSON.
    List,

    /// Write a built-in graph template as JSON.
    Show(TemplateShowArgs),
}

#[derive(Debug, Args)]
struct TargetAddArgs {
    /// Local name used to select this target.
    #[arg(value_name = "NAME")]
    name: String,

    /// Target origin URL.
    #[arg(long, value_name = "ORIGIN")]
    url: String,

    /// Use unauthenticated direct access instead of hosted authentication.
    #[arg(long)]
    direct: bool,
}

#[derive(Debug, Args)]
struct TargetNameArgs {
    /// Local target name.
    #[arg(value_name = "NAME")]
    name: String,
}

#[derive(Debug, Args)]
struct TargetServeArgs {
    /// IP socket address on which the target listens.
    #[arg(long, value_name = "ADDRESS")]
    listen: SocketAddr,

    /// Public HTTP(S) origin advertised to clients.
    #[arg(long, value_name = "ORIGIN")]
    public_origin: String,

    /// Directory that stores target state and run data.
    #[arg(long, value_name = "DIRECTORY")]
    storage: PathBuf,

    #[arg(long, value_name = "PATH", hide = true)]
    bootstrap_key_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TemplateName {
    /// A single general-purpose worker.
    SingleWorker,

    /// A review, validation, and optional delivery workflow for code changes.
    SoftwareChange,
}

#[derive(Debug, Args)]
#[group(id = "delivery_mode", multiple = false)]
struct DeliveryArgs {
    /// Materialize this template-owned delivery mode.
    #[arg(long, value_name = "MODE")]
    delivery: Option<String>,

    /// Materialize pull-request delivery for the software-change template.
    #[arg(long)]
    pr: bool,

    /// Materialize merge delivery for the software-change template.
    ///
    /// Named-target runs forward GH_TOKEN for the generated GitHub merge operation.
    #[arg(long)]
    ship: bool,
}

impl DeliveryArgs {
    fn selection(&self) -> (Option<&str>, bool, bool) {
        (self.delivery.as_deref(), self.pr, self.ship)
    }
}

#[derive(Debug, Args)]
struct TemplateShowArgs {
    /// Built-in graph template to render.
    #[arg(value_enum, value_name = "TEMPLATE")]
    template: TemplateName,

    #[command(flatten)]
    delivery: DeliveryArgs,
}

#[derive(Debug, Args)]
#[command(after_long_help = r#"RUNTIME CONFIGURATION
    The file is secret-free JSON. For example:

      {
        "harness": "codex",
        "provider": "openrouter",
        "size": "medium",
        "nodes": {
          "worker": {
            "kind": "agent",
            "model": "provider-model-id",
            "connections": {"openrouter": ["OPENROUTER_API_KEY"]}
          }
        }
      }

    Provider choices are codex/openai, codex/openrouter, codex/bedrock, claude/anthropic,
    claude/openrouter, and claude/bedrock. Known-incompatible harness/provider pairs are
    codex/anthropic and claude/openai. Model IDs are passed unchanged to the selected harness and
    provider; Zeroshot does not maintain or validate provider model catalogs.

    Sizes are small, medium, and large.

    Every executable graph node needs a same-named binding. Agent bindings require kind and model.
    Optional fields are effort (low, medium, high, xhigh, or max when supported), sessionScope
    (execution or node_instance), and connections. Each connection key maps to the exact
    environment variable names required by that node; never put values in this file.

    Use `zeroshot template show TEMPLATE` to inspect node names. With --pr or --ship, omit the
    template-owned delivery binding.

    --uniform-runtime-config requires harness, provider, and model. It accepts optional size,
    effort, sessionScope, and connections fields without nodes. Zeroshot expands that agent binding
    across every executable graph node and supplies graph-visible Git delivery bindings itself."#)]
struct RunArgs {
    /// Human-readable title recorded with the run.
    #[arg(long, value_name = "TITLE")]
    title: String,

    /// Load a custom graph specification from this JSON file.
    #[arg(long, value_name = "FILE")]
    graph: Option<PathBuf>,

    /// Materialize and run this built-in graph template.
    #[arg(long, value_enum, value_name = "TEMPLATE")]
    template: Option<TemplateName>,

    /// Load the graph's initial input from this JSON file.
    #[arg(long, value_name = "FILE")]
    input: PathBuf,

    /// Load an exact secret-free runtime plan from this JSON file.
    #[arg(long, value_name = "FILE")]
    runtime_config: Option<PathBuf>,

    /// Expand one secret-free agent runtime across every executable graph node.
    #[arg(long, value_name = "FILE")]
    uniform_runtime_config: Option<PathBuf>,

    /// Use a profile: NAME, local:NAME, user:NAME, or org:NAME.
    ///
    /// When no profile or inline graph/runtime is supplied, scoped defaults are checked.
    #[arg(long, value_name = "[SCOPE:]NAME")]
    profile: Option<String>,

    /// Run on this named target; if omitted, run locally. Named targets receive GH_TOKEN when set.
    ///
    /// The token is used for source checkout and Git delivery. A provider receives it only when
    /// the runtime configuration explicitly declares GH_TOKEN.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,

    /// GitHub repository in owner/name form. Requires --target.
    #[arg(long, value_name = "OWNER/NAME")]
    repository: Option<String>,

    /// Source branch to resolve for the named run. Requires --target.
    #[arg(long, value_name = "BRANCH")]
    branch: Option<String>,

    /// Exact source commit SHA. Requires --target.
    #[arg(long, value_name = "SHA")]
    revision: Option<String>,

    /// Stable idempotency key for safely retrying submission.
    #[arg(long, value_name = "KEY")]
    submission_key: Option<String>,

    /// Return after submission instead of following NDJSON run events.
    #[arg(short = 'd', long)]
    detach: bool,

    /// Validate and materialize the run without submitting it or contacting a target.
    #[arg(long)]
    validate_only: bool,

    #[command(flatten)]
    delivery: DeliveryArgs,
}

#[derive(Debug, Args)]
struct PlanFileArgs {
    /// Merge-plan manifest JSON file.
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

#[derive(Debug, Args)]
struct PlanSubmitArgs {
    /// Merge-plan manifest JSON file.
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// Submit to this named hosted target.
    #[arg(long, value_name = "NAME")]
    target: String,

    /// Stable idempotency key for safely retrying the atomic submission.
    #[arg(long, value_name = "KEY")]
    submission_key: String,

    /// Return after atomic submission instead of polling plan status.
    #[arg(short = 'd', long)]
    detach: bool,
}

#[derive(Debug, Args)]
struct PlanSelectorArgs {
    /// Immutable merge-plan ID.
    #[arg(value_name = "PLAN_ID")]
    plan_id: String,

    /// Use this named hosted target.
    #[arg(long, value_name = "NAME")]
    target: String,
}

#[derive(Debug, Args)]
struct TargetRouteArgs {
    /// Use this named target. If omitted, use the local controller.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
}

#[derive(Debug, Args)]
struct RunSelectorArgs {
    /// Public run ID.
    #[arg(value_name = "RUN_ID")]
    run_id: String,

    /// Use this named target. If omitted, use the local controller.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
}

#[derive(Debug, Args)]
struct RunWatchArgs {
    #[command(flatten)]
    run: RunSelectorArgs,

    /// Resume strictly after this durable cursor.
    #[arg(long, value_name = "CURSOR")]
    after: Option<String>,
}

#[derive(Debug, Args)]
struct RunLogsArgs {
    #[command(flatten)]
    run: RunSelectorArgs,

    /// Resume strictly after this durable cursor.
    #[arg(long, value_name = "CURSOR")]
    after: Option<String>,

    /// Return records only for this opaque execution selector.
    #[arg(long, value_name = "EXECUTION_REF")]
    execution: Option<String>,
}

#[derive(Debug, Args)]
struct AttachArgs {
    /// Public run ID.
    #[arg(value_name = "RUN_ID")]
    run_id: String,

    /// Execution reference emitted by the run.
    #[arg(value_name = "EXECUTION_REF")]
    execution: String,

    /// Use this named target. If omitted, use the local controller.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
}

#[path = "parser/convert.rs"]
mod convert;
#[path = "parser/profiles.rs"]
mod profiles;
use profiles::{ProfileCommand, ProfileNameArgs, ProfileScopeArg};
pub use convert::parse_native_v2_args;
