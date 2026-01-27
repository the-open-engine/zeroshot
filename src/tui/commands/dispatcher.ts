import { CommandContext, CommandResult, ParsedCommand } from "./types";
import {
  CommandDefinition,
  CommandRegistry,
  createCommandRegistry,
} from "./registry";
import { runListTasks, runShowStatus } from "./cli-compat";
import {
  generateClusterId,
  launchClusterFromIssue,
} from "../services/cluster-launcher";
import {
  normalizeProviderName,
  VALID_PROVIDERS,
} from "../../../lib/provider-names";

const { detectIdType } = require("../../../lib/id-detector");
const { detectRunInput } = require("../../../lib/start-cluster");

type ListOptions = {
  status?: string;
  limit?: number;
  verbose?: boolean;
};

const LIST_USAGE =
  "Usage: /list [--status <status>] [--limit <n>] [--verbose]";
const ISSUE_USAGE = "Usage: /issue <ref>";

function formatHelp(definitions: CommandDefinition[]): string {
  const lines = definitions.map((definition) => {
    const usage = definition.usage ? ` ${definition.usage}` : "";
    return `/${definition.name}${usage} - ${definition.description}`;
  });

  return ["Commands:", ...lines, "Keys: Esc back, Ctrl+C exit"].join(" ");
}

function parseListOptions(args: string[]):
  | { options: ListOptions }
  | { error: string } {
  const options: ListOptions = {};

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    if (arg === "--status" || arg === "-s") {
      const value = args[index + 1];
      if (!value) {
        return { error: LIST_USAGE };
      }
      options.status = value;
      index += 1;
      continue;
    }

    if (arg === "--limit" || arg === "-n") {
      const value = args[index + 1];
      if (!value) {
        return { error: LIST_USAGE };
      }
      const parsed = Number(value);
      if (!Number.isFinite(parsed) || parsed <= 0) {
        return { error: LIST_USAGE };
      }
      options.limit = parsed;
      index += 1;
      continue;
    }

    if (arg === "--verbose" || arg === "-v") {
      options.verbose = true;
      continue;
    }

    return { error: LIST_USAGE };
  }

  return { options };
}

async function handleList(args: string[]): Promise<CommandResult> {
  const parsed = parseListOptions(args);
  if ("error" in parsed) {
    return { tone: "error", message: parsed.error };
  }

  try {
    const result = await runListTasks(parsed.options);
    const message = result.output || "No tasks found.";

    if (result.exitCode !== 0) {
      return { tone: "error", message };
    }

    return { tone: "info", message };
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    return { tone: "error", message: `Error listing tasks: ${message}` };
  }
}

async function handleClusterStatus(clusterId: string): Promise<CommandResult> {
  try {
    const Orchestrator = require("../../../src/orchestrator");
    const orchestrator = await Orchestrator.create({ quiet: true });
    const status = orchestrator.getStatus(clusterId);
    const state = status.isZombie ? "zombie" : status.state;
    const message = `Cluster ${status.id}: ${state}. Agents: ${status.agents.length}. Messages: ${status.messageCount}. Created: ${new Date(
      status.createdAt
    ).toLocaleString()}.`;
    return { tone: "info", message };
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    return { tone: "error", message: `Error getting cluster status: ${message}` };
  }
}

async function handleStatus(args: string[]): Promise<CommandResult> {
  if (args.length === 0) {
    return { tone: "error", message: "Usage: /status <id>" };
  }

  const id = args[0];
  let type: "cluster" | "task" | null = null;

  try {
    type = detectIdType(id);
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    return { tone: "error", message: `Error checking id: ${message}` };
  }

  if (!type) {
    try {
      const taskResult = await runShowStatus(id);
      const message = taskResult.output || `No status output for ${id}.`;
      if (taskResult.exitCode === 0) {
        return { tone: "info", message };
      }
    } catch {
      // fall through to cluster check
    }

    try {
      const clusterResult = await handleClusterStatus(id);
      if (clusterResult.tone === "info") {
        return clusterResult;
      }
    } catch {
      // ignore cluster errors, report unknown id below
    }

    return { tone: "error", message: `Unknown id: ${id}.` };
  }

  if (type === "cluster") {
    return handleClusterStatus(id);
  }

  try {
    const result = await runShowStatus(id);
    const message = result.output || `No status output for ${id}.`;

    if (result.exitCode !== 0) {
      return { tone: "error", message };
    }

    return { tone: "info", message };
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    return { tone: "error", message: `Error getting status: ${message}` };
  }
}

async function handleIssue(
  args: string[],
  context: CommandContext
): Promise<CommandResult> {
  if (args.length === 0) {
    return { tone: "error", message: ISSUE_USAGE };
  }

  const ref = args.join(" ").trim();
  if (!ref) {
    return { tone: "error", message: ISSUE_USAGE };
  }

  const issueDeps = context.issueLaunchDeps ?? {};
  const detectRunInputImpl = issueDeps.detectRunInput ?? detectRunInput;

  let parsed: { issue?: string } | null = null;
  try {
    parsed = detectRunInputImpl(ref);
  } catch {
    return { tone: "error", message: `Invalid issue reference: ${ref}.` };
  }

  if (!parsed || typeof parsed !== "object" || !("issue" in parsed)) {
    return { tone: "error", message: `Invalid issue reference: ${ref}.` };
  }

  const generateClusterIdImpl =
    issueDeps.generateClusterId ?? generateClusterId;
  const launchClusterFromIssueImpl =
    issueDeps.launchClusterFromIssue ?? launchClusterFromIssue;
  const clusterId = generateClusterIdImpl();

  try {
    await launchClusterFromIssueImpl({
      ref,
      providerOverride: context.provider ?? null,
      clusterId,
    });
  } catch (error) {
    const message =
      error instanceof Error ? error.message : "Issue launch failed.";
    return { tone: "error", message: `Error starting issue: ${message}` };
  }

  context.setClusterId(clusterId);
  context.navigate("cluster");
  return { tone: "success", message: `Cluster ${clusterId} started.` };
}

function createBuiltInRegistry(): CommandRegistry {
  const registry = createCommandRegistry();

  registry.register({
    name: "help",
    description: "show commands",
    handler: () => ({
      tone: "info",
      message: formatHelp(registry.list()),
    }),
  });

  registry.register({
    name: "monitor",
    description: "open Monitor view",
    handler: (_args, context) => {
      context.navigate("monitor");
      return { tone: "success", message: "Opened Monitor view." };
    },
  });

  registry.register({
    name: "issue",
    description: "start from issue",
    usage: "<ref>",
    handler: (args, context) => handleIssue(args, context),
  });

  registry.register({
    name: "provider",
    description: "switch provider",
    usage: "<name>",
    handler: (args, context) => {
      if (args.length === 0) {
        return { tone: "error", message: "Usage: /provider <name>" };
      }
      const normalized = normalizeProviderName(args[0]);
      if (!VALID_PROVIDERS.includes(normalized)) {
        return {
          tone: "error",
          message: `Unknown provider: ${args[0]}. Valid: ${VALID_PROVIDERS.join(
            ", "
          )}.`,
        };
      }
      context.setProvider(normalized);
      return { tone: "success", message: `Provider set to ${normalized}.` };
    },
  });

  registry.register({
    name: "list",
    description: "list tasks",
    usage: "[--status <status>] [--limit <n>] [--verbose]",
    handler: (args) => handleList(args),
  });

  registry.register({
    name: "status",
    description: "show task or cluster status",
    usage: "<id>",
    handler: (args) => handleStatus(args),
  });

  registry.register({
    name: "quit",
    description: "exit the TUI",
    handler: (_args, context) => {
      context.exit();
      return { tone: "info", message: "Exiting..." };
    },
  });

  return registry;
}

let registry: CommandRegistry | null = null;

function getRegistry(): CommandRegistry {
  if (!registry) {
    registry = createBuiltInRegistry();
  }
  return registry;
}

export async function dispatchCommand(
  command: ParsedCommand,
  context: CommandContext
): Promise<CommandResult> {
  return getRegistry().dispatch(command, context);
}
