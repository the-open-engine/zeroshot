import { existsSync, statSync, readFileSync, openSync, readSync, closeSync } from 'fs';
import chalk from 'chalk';
import { getTask } from '../store.js';
import { isProcessRunning } from '../runner.js';
import { createRequire } from 'module';

// Import cluster's stream parser (shared between task and cluster)
const require = createRequire(import.meta.url);
const { parseChunk } = require('../../lib/stream-json-parser');

// Tool icons for different tool types
const TOOL_ICONS = {
  Read: '📖',
  Write: '📝',
  Edit: '✏️',
  Bash: '💻',
  Glob: '🔍',
  Grep: '🔎',
  WebFetch: '🌐',
  WebSearch: '🔎',
  Task: '🤖',
  TodoWrite: '📋',
  AskUserQuestion: '❓',
  BashOutput: '📤',
  KillShell: '🔪',
};

function getToolIcon(toolName) {
  return TOOL_ICONS[toolName] || '🔧';
}

// Format tool call input for display
function formatToolCall(toolName, input) {
  if (!input) return '';

  switch (toolName) {
    case 'Bash':
      return input.command ? `$ ${input.command}` : '';
    case 'Read':
      return input.file_path ? input.file_path.split('/').slice(-2).join('/') : '';
    case 'Write':
      return input.file_path ? `→ ${input.file_path.split('/').slice(-2).join('/')}` : '';
    case 'Edit':
      return input.file_path ? input.file_path.split('/').slice(-2).join('/') : '';
    case 'Glob':
      return input.pattern || '';
    case 'Grep':
      return input.pattern ? `/${input.pattern}/` : '';
    case 'WebFetch':
      return input.url ? input.url.substring(0, 50) : '';
    case 'WebSearch':
      return input.query ? `"${input.query}"` : '';
    case 'Task':
      return input.description || '';
    case 'TodoWrite':
      if (input.todos && Array.isArray(input.todos)) {
        const statusCounts = {};
        input.todos.forEach((todo) => {
          statusCounts[todo.status] = (statusCounts[todo.status] || 0) + 1;
        });
        const parts = Object.entries(statusCounts).map(
          ([status, count]) => `${count} ${status.replace('_', ' ')}`
        );
        return `${input.todos.length} todo${input.todos.length === 1 ? '' : 's'} (${parts.join(', ')})`;
      }
      return '';
    case 'AskUserQuestion':
      if (input.questions && Array.isArray(input.questions)) {
        const q = input.questions[0];
        const preview = q.question.substring(0, 50);
        return input.questions.length > 1
          ? `${input.questions.length} questions: "${preview}..."`
          : `"${preview}${q.question.length > 50 ? '...' : ''}"`;
      }
      return '';
    default:
      // For unknown tools, show first key-value pair
      const keys = Object.keys(input);
      if (keys.length > 0) {
        const val = String(input[keys[0]]).substring(0, 40);
        return val.length < String(input[keys[0]]).length ? val + '...' : val;
      }
      return '';
  }
}

// Format tool result for display
function formatToolResult(content, isError, toolName, toolInput) {
  if (!content) return isError ? 'error' : 'done';

  // For errors, show full message
  if (isError) {
    const firstLine = content.split('\n')[0].substring(0, 80);
    return chalk.red(firstLine);
  }

  // For TodoWrite, show the actual todo items
  if (toolName === 'TodoWrite' && toolInput?.todos && Array.isArray(toolInput.todos)) {
    const todos = toolInput.todos;
    if (todos.length === 0) return chalk.dim('no todos');
    if (todos.length === 1) {
      const status =
        todos[0].status === 'completed' ? '✓' : todos[0].status === 'in_progress' ? '⧗' : '○';
      return chalk.dim(
        `${status} ${todos[0].content.substring(0, 50)}${todos[0].content.length > 50 ? '...' : ''}`
      );
    }
    // Multiple todos - show first one as preview
    const status =
      todos[0].status === 'completed' ? '✓' : todos[0].status === 'in_progress' ? '⧗' : '○';
    return chalk.dim(
      `${status} ${todos[0].content.substring(0, 40)}... (+${todos.length - 1} more)`
    );
  }

  // For success, show summary
  const lines = content.split('\n').filter((l) => l.trim());
  if (lines.length === 0) return 'done';
  if (lines.length === 1) {
    const line = lines[0].substring(0, 60);
    return chalk.dim(line.length < lines[0].length ? line + '...' : line);
  }
  // Multiple lines - show count
  return chalk.dim(`${lines.length} lines`);
}

// Line buffer for accumulating text that streams without newlines
let lineBuffer = '';
let currentToolCall = null;

function resetState() {
  lineBuffer = '';
  currentToolCall = null;
}

// Flush pending text in buffer
function flushLineBuffer() {
  if (lineBuffer.trim()) {
    process.stdout.write(lineBuffer);
    if (!lineBuffer.endsWith('\n')) {
      process.stdout.write('\n');
    }
  }
  lineBuffer = '';
}

// Accumulate text and print complete lines
function accumulateText(text) {
  lineBuffer += text;

  // Print complete lines, keep incomplete in buffer
  const lines = lineBuffer.split('\n');
  if (lines.length > 1) {
    // Print all complete lines
    for (let i = 0; i < lines.length - 1; i++) {
      console.log(lines[i]);
    }
    // Keep incomplete line in buffer
    lineBuffer = lines[lines.length - 1];
  }
}

// Process parsed events and output formatted content
function processEvent(event) {
  switch (event.type) {
    case 'text':
      accumulateText(event.text);
      break;

    case 'thinking':
    case 'thinking_start':
      if (event.text) {
        console.log(chalk.dim.italic(event.text));
      } else if (event.type === 'thinking_start') {
        console.log(chalk.dim.italic('💭 thinking...'));
      }
      break;

    case 'tool_start':
      flushLineBuffer();
      break;

    case 'tool_call':
      flushLineBuffer();
      const icon = getToolIcon(event.toolName);
      const toolDesc = formatToolCall(event.toolName, event.input);
      console.log(`${icon} ${chalk.cyan(event.toolName)} ${chalk.dim(toolDesc)}`);
      currentToolCall = { toolName: event.toolName, input: event.input };
      break;

    case 'tool_input':
      // Streaming tool input JSON - skip (shown in tool_call)
      break;

    case 'tool_result':
      const status = event.isError ? chalk.red('✗') : chalk.green('✓');
      const resultDesc = formatToolResult(
        event.content,
        event.isError,
        currentToolCall?.toolName,
        currentToolCall?.input
      );
      console.log(`  ${status} ${resultDesc}`);
      break;

    case 'result':
      flushLineBuffer();
      if (event.error) {
        console.log(chalk.red(`\n✗ ERROR: ${event.error}`));
      } else {
        console.log(chalk.green(`\n✓ Completed`));
        if (event.cost) {
          console.log(chalk.dim(`   Cost: $${event.cost.toFixed(4)}`));
        }
        if (event.duration) {
          const mins = Math.floor(event.duration / 60000);
          const secs = Math.floor((event.duration % 60000) / 1000);
          console.log(chalk.dim(`   Duration: ${mins}m ${secs}s`));
        }
      }
      break;

    case 'block_end':
      // Content block ended
      break;

    case 'multi':
      // Multiple events
      if (event.events) {
        for (const e of event.events) {
          processEvent(e);
        }
      }
      break;
  }
}

// Parse a raw log line (may have timestamp prefix)
function parseLogLine(line) {
  let trimmed = line.trim();
  if (!trimmed) return [];

  // Strip timestamp prefix if present: [1234567890]{...} -> {...}
  const timestampMatch = trimmed.match(/^\[\d+\](.*)$/);
  if (timestampMatch) {
    trimmed = timestampMatch[1];
  }

  // Non-JSON lines output as-is
  if (!trimmed.startsWith('{')) {
    return [{ type: 'text', text: trimmed + '\n' }];
  }

  // Parse JSON using cluster's parser
  return parseChunk(trimmed);
}

export async function showLogs(taskId, options = {}) {
  const task = getTask(taskId);

  if (!task) {
    console.log(chalk.red(`Task not found: ${taskId}`));
    process.exit(1);
  }

  if (!existsSync(task.logFile)) {
    console.log(chalk.yellow('Log file not found (task may still be starting).'));
    return;
  }

  const lines = options.lines || 50;
  const follow = options.follow;
  const watch = options.watch;

  // IMPORTANT: -f should stream logs (like tail -f), -w launches interactive TUI
  // This prevents confusion when users expect tail-like behavior
  if (watch) {
    // Use TUI for watch mode (interactive interface)
    const { default: TaskLogsTUI } = await import('../tui/index.js');
    const tui = new TaskLogsTUI({
      taskId: task.id,
      logFile: task.logFile,
      taskInfo: {
        status: task.status,
        createdAt: task.createdAt,
        prompt: task.prompt,
      },
      pid: task.pid,
    });
    await tui.start();
  } else if (follow) {
    // Stream logs continuously (like tail -f) - show last N lines first
    await tailFollow(task.logFile, task.pid, lines);
  } else {
    await tailLines(task.logFile, lines);
  }
}

function tailLines(file, n) {
  resetState();
  const rawContent = readFileSync(file, 'utf-8');
  const rawLines = rawContent.split('\n');

  // Parse and process all events
  const allEvents = [];
  for (const line of rawLines) {
    const events = parseLogLine(line);
    allEvents.push(...events);
  }

  // Tail to last n events
  const tailedEvents = allEvents.slice(-n);
  for (const event of tailedEvents) {
    processEvent(event);
  }
  flushLineBuffer();
}

async function tailFollow(file, pid, lines = 50) {
  resetState();
  // First, output last N events (like tail -f behavior)
  const rawContent = readFileSync(file, 'utf-8');
  const rawLines = rawContent.split('\n');

  // Parse all events first
  const allEvents = [];
  for (const line of rawLines) {
    const events = parseLogLine(line);
    allEvents.push(...events);
  }

  // Output only the last N events
  const tailedEvents = allEvents.slice(-lines);
  for (const event of tailedEvents) {
    processEvent(event);
  }
  flushLineBuffer();

  // Poll for changes (more reliable than fs.watch)
  let lastSize = statSync(file).size;
  let noChangeCount = 0;

  const interval = setInterval(() => {
    try {
      const currentSize = statSync(file).size;

      if (currentSize > lastSize) {
        // Read new content
        const buffer = Buffer.alloc(currentSize - lastSize);
        const fd = openSync(file, 'r');
        readSync(fd, buffer, 0, buffer.length, lastSize);
        closeSync(fd);

        // Parse and output new lines
        const newLines = buffer.toString().split('\n');
        for (const line of newLines) {
          const events = parseLogLine(line);
          for (const event of events) {
            processEvent(event);
          }
        }
        flushLineBuffer();

        lastSize = currentSize;
        noChangeCount = 0;
      } else {
        noChangeCount++;

        // Check if process is still running after 5 seconds of no output
        if (noChangeCount >= 10 && pid && !isProcessRunning(pid)) {
          // Read any final content
          const finalSize = statSync(file).size;
          if (finalSize > lastSize) {
            const buffer = Buffer.alloc(finalSize - lastSize);
            const fd = openSync(file, 'r');
            readSync(fd, buffer, 0, buffer.length, lastSize);
            closeSync(fd);

            const finalLines = buffer.toString().split('\n');
            for (const line of finalLines) {
              const events = parseLogLine(line);
              for (const event of events) {
                processEvent(event);
              }
            }
            flushLineBuffer();
          }

          console.log(chalk.dim('\n--- Task completed ---'));
          clearInterval(interval);
          process.exit(0);
        }
      }
    } catch (err) {
      // File might have been deleted
      console.log(chalk.red(`\nError reading log: ${err.message}`));
      clearInterval(interval);
      process.exit(1);
    }
  }, 500); // Poll every 500ms

  // Keep alive until Ctrl+C
  process.on('SIGINT', () => {
    clearInterval(interval);
    console.log(chalk.dim('\nStopped following.'));
    process.exit(0);
  });

  // Keep process running
  await new Promise(() => {});
}
