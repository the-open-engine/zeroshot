import { CommandContext, CommandResult, ParsedCommand } from "./types.js";

export type CommandHandler = (
  args: string[],
  context: CommandContext
) => Promise<CommandResult> | CommandResult;

export type CommandDefinition = {
  name: string;
  description: string;
  usage?: string;
  handler: CommandHandler;
};

export type CommandRegistry = {
  register: (definition: CommandDefinition) => void;
  list: () => CommandDefinition[];
  dispatch: (
    command: ParsedCommand,
    context: CommandContext
  ) => Promise<CommandResult>;
};

export function createCommandRegistry(): CommandRegistry {
  const commands = new Map<string, CommandDefinition>();

  function register(definition: CommandDefinition): void {
    const key = definition.name.toLowerCase();
    if (!key) {
      throw new Error("Command name is required.");
    }
    if (commands.has(key)) {
      throw new Error(`Command already registered: ${key}`);
    }
    commands.set(key, { ...definition, name: key });
  }

  function list(): CommandDefinition[] {
    return Array.from(commands.values()).sort((a, b) =>
      a.name.localeCompare(b.name)
    );
  }

  async function dispatch(
    command: ParsedCommand,
    context: CommandContext
  ): Promise<CommandResult> {
    const key = command.name.toLowerCase();
    const definition = commands.get(key);
    if (!definition) {
      return {
        tone: "error",
        message: `Unknown command: /${command.name || ""}. Type /help for commands.`,
      };
    }

    return await definition.handler(command.args, context);
  }

  return { register, list, dispatch };
}
