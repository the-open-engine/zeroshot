import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

function settingsFilePath(): string {
  return process.env.ZEROSHOT_SETTINGS_FILE || join(homedir(), '.zeroshot', 'settings.json');
}

export function readConfiguredClaudeCommand(): string {
  if (process.env.ZEROSHOT_CLAUDE_COMMAND?.trim()) {
    return process.env.ZEROSHOT_CLAUDE_COMMAND;
  }

  const settingsPath = settingsFilePath();
  if (!existsSync(settingsPath)) return 'claude';

  try {
    const settings: unknown = JSON.parse(readFileSync(settingsPath, 'utf8'));
    if (
      settings !== null &&
      typeof settings === 'object' &&
      'claudeCommand' in settings &&
      typeof settings.claudeCommand === 'string' &&
      settings.claudeCommand.trim()
    ) {
      return settings.claudeCommand;
    }
  } catch {
    return 'claude';
  }

  return 'claude';
}

export function resolveClaudeCommand(): {
  readonly command: string;
  readonly args: readonly string[];
} {
  const parts = readConfiguredClaudeCommand()
    .trim()
    .split(/\s+/)
    .filter((part) => part.length > 0);
  return {
    command: parts[0] ?? 'claude',
    args: parts.slice(1),
  };
}
