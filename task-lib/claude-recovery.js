import { existsSync, readdirSync, readFileSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';

export const STREAMING_MODE_ERROR = 'only prompt commands are supported in streaming mode';

export function detectStreamingModeError(line) {
  const trimmed = typeof line === 'string' ? line.trim() : '';
  if (!trimmed.startsWith('{')) return null;

  try {
    const parsed = JSON.parse(trimmed);
    if (
      parsed &&
      parsed.type === 'result' &&
      parsed.is_error === true &&
      Array.isArray(parsed.errors) &&
      parsed.errors.includes(STREAMING_MODE_ERROR) &&
      typeof parsed.session_id === 'string'
    ) {
      return {
        sessionId: parsed.session_id,
        line: trimmed,
      };
    }
  } catch {
    // Ignore parse errors - not JSON
  }

  return null;
}

function findSessionJsonlPath(sessionId) {
  const claudeDir = process.env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude');
  const projectsDir = join(claudeDir, 'projects');
  if (!existsSync(projectsDir)) return null;

  const target = `${sessionId}.jsonl`;
  const queue = [projectsDir];

  while (queue.length > 0) {
    const dir = queue.pop();
    if (!dir) continue;

    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      continue;
    }

    for (const entry of entries) {
      if (entry.isFile() && entry.name === target) {
        return join(dir, entry.name);
      }
      if (entry.isDirectory()) {
        queue.push(join(dir, entry.name));
      }
    }
  }

  return null;
}

export function recoverStructuredOutput(sessionId) {
  const jsonlPath = findSessionJsonlPath(sessionId);
  if (!jsonlPath) return null;

  const fileContents = readJsonlFile(jsonlPath);
  if (!fileContents) return null;

  const { structuredOutput, usage } = findStructuredOutput(fileContents);

  if (!structuredOutput) return null;

  return {
    payload: buildStructuredOutputPayload(sessionId, structuredOutput, usage),
    sourcePath: jsonlPath,
  };
}

function readJsonlFile(jsonlPath) {
  try {
    return readFileSync(jsonlPath, 'utf8');
  } catch {
    return null;
  }
}

function findStructuredOutput(fileContents) {
  const lines = fileContents.split('\n');
  let structuredOutput = null;
  let usage = null;

  for (const line of lines) {
    const entry = parseJsonLine(line);
    if (!entry) {
      continue;
    }

    const extracted = extractStructuredOutputFromEntry(entry);
    if (extracted) {
      structuredOutput = extracted.structuredOutput;
      usage = extracted.usage;
    }
  }

  return { structuredOutput, usage };
}

function parseJsonLine(line) {
  if (!line.trim()) return null;
  try {
    return JSON.parse(line);
  } catch {
    // Skip invalid JSON lines
    return null;
  }
}

function extractStructuredOutputFromEntry(entry) {
  const message = entry?.message;
  const content = message?.content;
  if (!Array.isArray(content)) return null;

  let structuredOutput = null;
  let usage = null;
  for (const block of content) {
    if (block?.type === 'tool_use' && block?.name === 'StructuredOutput' && block?.input) {
      structuredOutput = block.input;
      usage = message?.usage && typeof message.usage === 'object' ? message.usage : null;
    }
  }

  if (!structuredOutput) {
    return null;
  }

  return { structuredOutput, usage };
}

function buildStructuredOutputPayload(sessionId, structuredOutput, usage) {
  const payload = {
    type: 'result',
    subtype: 'success',
    is_error: false,
    structured_output: structuredOutput,
    session_id: sessionId,
  };

  if (usage) {
    payload.usage = usage;
  }

  return payload;
}
