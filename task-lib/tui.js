/**
 * Task TUI - Interactive task viewer
 *
 * Features:
 * - List all tasks
 * - Navigate with arrow keys
 * - Press Enter to view logs
 * - Press Esc to go back to list
 */

import blessed from 'blessed';
import { loadTasks } from './store.js';
import { isProcessRunning } from './runner.js';
import fs from 'fs';
import path from 'path';
import os from 'os';

// Parse a single log line from JSON stream format
function parseLogLine(line) {
  let trimmed = line.trim();

  // Strip timestamp prefix if present: [1234567890]{...} -> {...}
  const timestampMatch = trimmed.match(/^\[\d+\](.*)$/);
  if (timestampMatch) {
    trimmed = timestampMatch[1];
  }

  // Keep non-JSON lines
  if (!trimmed.startsWith('{')) {
    return trimmed ? trimmed + '\n' : '';
  }

  // Parse JSON and extract relevant info
  try {
    const event = JSON.parse(trimmed);

    // Extract text from content_block_delta
    if (event.type === 'stream_event' && event.event?.type === 'content_block_delta') {
      return event.event?.delta?.text || '';
    }
    // Extract tool use info
    else if (event.type === 'stream_event' && event.event?.type === 'content_block_start') {
      const block = event.event?.content_block;
      if (block?.type === 'tool_use' && block?.name) {
        return `\n[Tool: ${block.name}]\n`;
      }
    }
    // Extract assistant messages
    else if (event.type === 'assistant' && event.message?.content) {
      let output = '';
      for (const content of event.message.content) {
        if (content.type === 'text') {
          output += content.text;
        }
      }
      return output;
    }
    // Extract final result
    else if (event.type === 'result') {
      if (event.is_error) {
        return `\n[ERROR] ${event.result || 'Unknown error'}\n`;
      }
    }
  } catch {
    // Not JSON or parse error - skip
  }

  return '';
}

class TaskTUI {
  constructor(options = {}) {
    this.tasks = [];
    this.selectedIndex = 0;
    this.viewMode = 'list'; // 'list' or 'detail'
    this.selectedTask = null;
    this.initialScrollDone = false;
    this.refreshRate = options.refreshRate || 1000;
  }

  start() {
    // Create screen
    this.screen = blessed.screen({
      smartCSR: true,
      title: 'Vibe Task Watch',
      dockBorders: true,
      fullUnicode: true,
    });

    // Create main list view
    this.listBox = blessed.list({
      parent: this.screen,
      top: 0,
      left: 0,
      width: '100%',
      height: '100%-2',
      keys: true,
      vi: true,
      mouse: true,
      border: {
        type: 'line',
      },
      style: {
        selected: {
          bg: 'blue',
          fg: 'white',
          bold: true,
        },
        border: {
          fg: 'cyan',
        },
      },
      tags: true,
    });

    // Create detail view (hidden initially)
    this.detailBox = blessed.box({
      parent: this.screen,
      top: 0,
      left: 0,
      width: '100%',
      height: '100%-2',
      border: {
        type: 'line',
      },
      style: {
        border: {
          fg: 'cyan',
        },
      },
      scrollable: true,
      alwaysScroll: true,
      keys: true,
      vi: true,
      mouse: true,
      scrollbar: {
        ch: '│',
        style: {
          bg: 'cyan',
        },
      },
      tags: false,
      hidden: true,
    });

    // Status bar
    this.statusBar = blessed.box({
      parent: this.screen,
      bottom: 0,
      left: 0,
      width: '100%',
      height: 2,
      content: '',
      tags: true,
      style: {
        bg: 'blue',
        fg: 'white',
      },
    });

    // Setup keybindings
    this.setupKeybindings();

    // Initial render
    this.refreshData();
    this.render();

    // Start polling
    this.pollInterval = setInterval(() => {
      this.refreshData();
      this.render();
    }, this.refreshRate);

    // Render screen
    this.screen.render();

    // Focus on list after layout is established
    this.listBox.focus();
  }

  setupKeybindings() {
    // Quit
    this.screen.key(['q', 'C-c'], () => {
      if (this.pollInterval) {
        clearInterval(this.pollInterval);
      }
      process.exit(0);
    });

    // Enter - view details
    this.listBox.key(['enter', 'space'], () => {
      if (this.tasks.length > 0) {
        this.selectedTask = this.tasks[this.selectedIndex];
        this.viewMode = 'detail';
        this.initialScrollDone = false;
        this.listBox.hide();
        this.detailBox.show();
        this.screen.render();
        setImmediate(() => {
          this.detailBox.focus();
          this.render();
        });
      }
    });

    // Escape - back to list
    this.screen.key(['escape'], () => {
      if (this.viewMode === 'detail') {
        this.viewMode = 'list';
        this.detailBox.hide();
        this.listBox.show();
        this.screen.render();
        setImmediate(() => {
          this.listBox.focus();
          this.render();
        });
      }
    });

    // Arrow keys for list
    this.listBox.key(['up', 'k'], () => {
      if (this.selectedIndex > 0) {
        this.selectedIndex--;
        this.listBox.select(this.selectedIndex);
        this.render();
      }
    });

    this.listBox.key(['down', 'j'], () => {
      if (this.selectedIndex < this.tasks.length - 1) {
        this.selectedIndex++;
        this.listBox.select(this.selectedIndex);
        this.render();
      }
    });

    // Scroll in detail view
    this.detailBox.key(['up', 'k'], () => {
      this.detailBox.scroll(-1);
      this.screen.render();
    });

    this.detailBox.key(['down', 'j'], () => {
      this.detailBox.scroll(1);
      this.screen.render();
    });

    this.detailBox.key(['pageup', 'u'], () => {
      this.detailBox.scroll(-10);
      this.screen.render();
    });

    this.detailBox.key(['pagedown', 'd'], () => {
      this.detailBox.scroll(10);
      this.screen.render();
    });
  }

  refreshData() {
    const tasks = loadTasks();
    this.tasks = Object.values(tasks);

    // Sort by creation date, newest first
    this.tasks.sort((a, b) => new Date(b.createdAt) - new Date(a.createdAt));

    // Verify running status
    for (const task of this.tasks) {
      if (task.status === 'running' && !isProcessRunning(task.pid)) {
        task.status = 'stale';
      }
    }
  }

  render() {
    if (this.viewMode === 'list') {
      this.renderList();
    } else {
      this.renderDetail();
    }
  }

  renderList() {
    const items = this.tasks.map((task) => {
      const statusIcon =
        {
          running: '{blue-fg}●{/}',
          completed: '{green-fg}●{/}',
          failed: '{red-fg}●{/}',
          stale: '{yellow-fg}●{/}',
        }[task.status] || '{gray-fg}●{/}';

      const age = this.getAge(task.createdAt);
      const cwd = task.cwd.replace(os.homedir(), '~');

      return `${statusIcon} {cyan-fg}${task.id.padEnd(25)}{/} {gray-fg}${task.status.padEnd(10)} ${age.padEnd(10)} ${cwd}{/}`;
    });

    this.listBox.setItems(items);
    this.listBox.setLabel(
      ` Tasks (${this.tasks.length}) - ↑↓ navigate, Enter to view logs, q to quit `
    );

    // Update status bar
    if (this.tasks.length > 0) {
      const task = this.tasks[this.selectedIndex];
      this.statusBar.setContent(
        `  Selected: {cyan-fg}${task.id}{/} | Status: ${this.getStatusColor(task.status)} | Press Enter to view logs`
      );
    } else {
      this.statusBar.setContent('  No tasks found');
    }

    this.screen.render();
  }

  renderDetail() {
    if (!this.selectedTask) return;

    const task = this.selectedTask;

    // Load and parse log file
    const logPath = path.join(os.homedir(), '.claude-zeroshot', 'logs', `${task.id}.log`);
    let content = '';

    if (fs.existsSync(logPath)) {
      try {
        const rawContent = fs.readFileSync(logPath, 'utf8');
        const lines = rawContent.split('\n');

        // Parse JSON stream and extract human-readable content
        for (const line of lines) {
          const parsed = parseLogLine(line);
          if (parsed) content += parsed;
        }

        // Strip ANSI codes for clean display
        // eslint-disable-next-line no-control-regex
        content = content.replace(/\x1b\[[0-9;]*m/g, '');
      } catch (error) {
        content = `Error reading log: ${error.message}`;
      }
    } else {
      content = 'No log file found';
    }

    this.detailBox.setContent(content);
    this.detailBox.setLabel(` ${task.id} | ${task.status} | ↑↓ scroll, Esc back, q quit `);

    // Update status bar
    this.statusBar.setContent(`  ${task.id} | Esc to go back`);

    // Scroll to bottom only on first view
    if (!this.initialScrollDone) {
      this.detailBox.setScrollPerc(100);
      this.initialScrollDone = true;
    }

    this.screen.render();
  }

  getAge(dateStr) {
    const diff = Date.now() - new Date(dateStr).getTime();
    const mins = Math.floor(diff / 60000);
    const hours = Math.floor(mins / 60);
    const days = Math.floor(hours / 24);

    if (days > 0) return `${days}d ago`;
    if (hours > 0) return `${hours}h ago`;
    if (mins > 0) return `${mins}m ago`;
    return 'just now';
  }

  getStatusColor(status) {
    const colors = {
      running: '{blue-fg}running{/}',
      completed: '{green-fg}completed{/}',
      failed: '{red-fg}failed{/}',
      stale: '{yellow-fg}stale{/}',
    };
    return colors[status] || status;
  }
}

export default TaskTUI;
