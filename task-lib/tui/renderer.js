/**
 * TUI Renderer for Task Logs
 * Transforms task data and log events into widget updates
 */

import { formatTimestamp, formatBytes, formatCPU, stateIcon, parseEvent } from './formatters.js';

class Renderer {
  /**
   * Create renderer instance
   * @param {object} widgets - Widget objects from layout
   * @param {object} screen - Blessed screen instance
   */
  constructor(widgets, screen) {
    if (!widgets) {
      throw new Error('Renderer requires widgets object from layout');
    }
    if (!screen) {
      throw new Error('Renderer requires screen instance');
    }

    this.widgets = widgets;
    this.screen = screen;
  }

  /**
   * Render task info box
   * @param {object} taskInfo - Task metadata
   * @param {object} stats - CPU/memory stats
   */
  renderTaskInfo(taskInfo, stats = {}) {
    if (!taskInfo) return;

    const icon = stateIcon(taskInfo.status || 'unknown');
    const runtime = taskInfo.createdAt ? formatTimestamp(Date.now() - taskInfo.createdAt) : '-';
    const cpu = stats.cpu !== undefined ? formatCPU(stats.cpu) : '0.0%';
    const memory = stats.memory !== undefined ? formatBytes(stats.memory) : '0 B';

    const content = [
      `${icon} {bold}Status:{/bold} {white-fg}${taskInfo.status || 'unknown'}{/}`,
      `{bold}Runtime:{/bold} {white-fg}${runtime}{/}`,
      `{bold}CPU:{/bold} {white-fg}${cpu}{/}  {bold}Memory:{/bold} {white-fg}${memory}{/}`,
      taskInfo.prompt
        ? `{bold}Task:{/bold} {gray-fg}${taskInfo.prompt.substring(0, 80)}${taskInfo.prompt.length > 80 ? '...' : ''}{/}`
        : '',
    ]
      .filter(Boolean)
      .join('  ');

    if (this.widgets.taskInfoBox && this.widgets.taskInfoBox.setContent) {
      this.widgets.taskInfoBox.setContent(content);
    }
  }

  /**
   * Render log entry to logs widget
   * @param {string} line - Raw log line
   */
  renderLogLine(line) {
    if (!line) return;

    const event = parseEvent(line);
    if (!event) return;

    let logMessage = '';

    switch (event.type) {
      case 'text':
        // Plain text output
        logMessage = event.text;
        break;

      case 'tool':
        // Tool invocation
        logMessage = `{cyan-fg}[Tool: ${event.toolName}]{/}`;
        break;

      case 'error':
        // Error message
        logMessage = `{red-fg}[ERROR] ${event.text}{/}`;
        break;

      case 'raw':
        // Raw non-JSON line
        logMessage = `{gray-fg}${event.text}{/}`;
        break;

      default:
        return;
    }

    if (this.widgets.logsBox && this.widgets.logsBox.log) {
      this.widgets.logsBox.log(logMessage);
    }
  }

  /**
   * Render multiple log lines at once
   * @param {string[]} lines - Array of log lines
   */
  renderLogLines(lines) {
    if (!Array.isArray(lines)) return;

    for (const line of lines) {
      this.renderLogLine(line);
    }
  }

  /**
   * Trigger screen render
   */
  render() {
    if (this.screen && this.screen.render) {
      this.screen.render();
    }
  }
}

export default Renderer;
