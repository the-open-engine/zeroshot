import { ParsedInput } from "./types";

export function parseInput(rawInput: string): ParsedInput {
  const raw = rawInput ?? "";
  const trimmed = raw.trim();

  if (!trimmed) {
    return { type: "empty", raw };
  }

  if (trimmed.startsWith("/")) {
    const withoutSlash = trimmed.slice(1).trim();
    if (!withoutSlash) {
      return { type: "command", name: "", args: [], raw };
    }
    const parts = withoutSlash.split(/\s+/);
    const name = parts[0].toLowerCase();
    const args = parts.slice(1);
    return { type: "command", name, args, raw };
  }

  return { type: "text", text: trimmed, raw };
}
