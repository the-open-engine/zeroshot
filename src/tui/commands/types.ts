import { ViewId } from "../view-stack";

export type ParsedCommand = {
  type: "command";
  name: string;
  args: string[];
  raw: string;
};

export type ParsedText = {
  type: "text";
  text: string;
  raw: string;
};

export type ParsedEmpty = {
  type: "empty";
  raw: string;
};

export type ParsedInput = ParsedCommand | ParsedText | ParsedEmpty;

export type CommandResultTone = "info" | "success" | "error";

export type CommandResult = {
  tone: CommandResultTone;
  message: string;
};

export type CommandHandler = (
  args: string[],
  context: CommandContext
) => Promise<CommandResult> | CommandResult;

export type CommandDispatcher = (
  command: ParsedCommand,
  context: CommandContext
) => Promise<CommandResult>;

export type IssueLaunchDeps = {
  generateClusterId?: () => string;
  launchClusterFromIssue?: (args: {
    ref: string;
    providerOverride?: string | null;
    clusterId?: string;
  }) => Promise<{ clusterId: string }>;
  detectRunInput?: (input: string) => {
    issue?: string;
    text?: string;
    file?: string;
  };
};

export type CommandContext = {
  navigate: (view: ViewId) => void;
  setProvider: (provider: string | null) => void;
  setClusterId: (clusterId: string | null) => void;
  provider: string | null;
  issueLaunchDeps?: IssueLaunchDeps;
  exit: () => void;
};
