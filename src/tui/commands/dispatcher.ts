import { CommandContext, CommandResult, ParsedCommand } from "./types";
import {
  normalizeProviderName,
  VALID_PROVIDERS,
} from "../../../lib/provider-names";

function formatHelp(): string {
  return [
    "Commands:",
    "/help - show commands",
    "/monitor - open Monitor view",
    "/issue <ref> - start from issue (stub)",
    "/provider <name> - switch provider",
    "/quit - exit the TUI",
    "Keys: Esc back, Ctrl+C exit",
  ].join(" ");
}

export function dispatchCommand(
  command: ParsedCommand,
  context: CommandContext
): CommandResult {
  const { name, args } = command;

  switch (name) {
    case "help":
      return { tone: "info", message: formatHelp() };
    case "monitor":
      context.navigate("monitor");
      return { tone: "success", message: "Opened Monitor view." };
    case "issue":
      if (args.length === 0) {
        return { tone: "error", message: "Usage: /issue <ref>" };
      }
      return {
        tone: "info",
        message: `Issue launch is not implemented yet. Placeholder for: ${args.join(" ")}.`,
      };
    case "provider":
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
    case "quit":
      context.exit();
      return { tone: "info", message: "Exiting..." };
    default:
      return {
        tone: "error",
        message: `Unknown command: /${name || ""}. Type /help for commands.`,
      };
  }
}
