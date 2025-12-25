/**
 * TUI - Task Logs Dashboard
 *
 * Coordinates:
 * - Screen and layout
 * - Log file polling
 * - Rendering
 * - Keybindings
 */

import blessed from 'blessed';
import { createLayout } from './layout.js';
import Renderer from './renderer.js';
import { statSync, openSync, readSync, closeSync } from 'fs';

class TaskLogsTUI {
  constructor(options) {
    this.taskId = options.taskId;
    this.logFile = options.logFile;
    this.taskInfo = options.taskInfo;
    this.pid = options.pid;

    // State
    this.lastSize = 0;
    this.pollInterval = null;
    this.widgets = null;
    this.screen = null;
    this.renderer = null;
    this.resourceStats = { cpu: 0, memory: 0 };
  }

  async start() {
    // Create screen
    this.screen = blessed.screen({
      smartCSR: true,
      title: `Task Logs: ${this.taskId}`,
      dockBorders: true,
      fullUnicode: true,
    });

    // Create layout
    this.widgets = createLayout(this.screen, this.taskId);

    // Create renderer
    this.renderer = new Renderer(this.widgets, this.screen);

    // Setup keybindings
    this._setupKeybindings();

    // Render initial task info
    this.renderer.renderTaskInfo(this.taskInfo, this.resourceStats);

    // Read existing log content
    await this._readExistingLogs();

    // Start polling for new content
    this._startPolling();

    // Initial render
    this.screen.render();
  }

  _setupKeybindings() {
    // Quit on 'q' or Ctrl+C
    this.screen.key(['q', 'C-c'], () => {
      this.exit();
    });

    // Scroll with arrow keys (blessed handles this automatically for logsBox)
    // Just ensure logsBox has focus
    this.widgets.logsBox.focus();
  }

  _readExistingLogs() {
    try {
      const fd = openSync(this.logFile, 'r');
      const size = statSync(this.logFile).size;

      if (size > 0) {
        const buffer = Buffer.alloc(size);
        readSync(fd, buffer, 0, size, 0);
        closeSync(fd);

        const content = buffer.toString();
        const lines = content.split('\n').filter((l) => l.trim());

        this.renderer.renderLogLines(lines);
        this.screen.render();
      }

      this.lastSize = size;
    } catch {
      // File might not exist yet
      this.lastSize = 0;
    }
  }

  _startPolling() {
    let noChangeCount = 0;

    this.pollInterval = setInterval(() => {
      try {
        const currentSize = statSync(this.logFile).size;

        if (currentSize > this.lastSize) {
          // Read new content
          const buffer = Buffer.alloc(currentSize - this.lastSize);
          const fd = openSync(this.logFile, 'r');
          readSync(fd, buffer, 0, buffer.length, this.lastSize);
          closeSync(fd);

          // Parse and render new lines
          const newContent = buffer.toString();
          const newLines = newContent.split('\n').filter((l) => l.trim());

          this.renderer.renderLogLines(newLines);
          this.screen.render();

          this.lastSize = currentSize;
          noChangeCount = 0;
        } else {
          noChangeCount++;

          // Check if process is still running after 5 seconds of no output
          if (noChangeCount >= 10 && this.pid && !this._isProcessRunning(this.pid)) {
            // Read any final content
            const finalSize = statSync(this.logFile).size;
            if (finalSize > this.lastSize) {
              const buffer = Buffer.alloc(finalSize - this.lastSize);
              const fd = openSync(this.logFile, 'r');
              readSync(fd, buffer, 0, buffer.length, this.lastSize);
              closeSync(fd);

              const finalContent = buffer.toString();
              const finalLines = finalContent.split('\n').filter((l) => l.trim());

              this.renderer.renderLogLines(finalLines);
            }

            // Update task status to completed
            this.taskInfo.status = 'completed';
            this.renderer.renderTaskInfo(this.taskInfo, this.resourceStats);
            this.screen.render();

            // Show completion message
            this.widgets.logsBox.log('{dim}--- Task completed ---{/}');
            this.screen.render();
          }
        }

        // Update resource stats periodically
        if (this.pid && this._isProcessRunning(this.pid)) {
          this.resourceStats = this._getResourceStats(this.pid);
          this.renderer.renderTaskInfo(this.taskInfo, this.resourceStats);
          this.screen.render();
        }
      } catch (error) {
        // File might have been deleted
        this.widgets.logsBox.log(`{red-fg}Error reading log: ${error.message}{/}`);
        this.screen.render();
      }
    }, 500); // Poll every 500ms
  }

  _isProcessRunning(pid) {
    if (!pid) return false;

    try {
      // Send signal 0 (no-op) to check if process exists
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  }

  _getResourceStats(_pid) {
    // This is a simplified version - full implementation would use pidusage or similar
    // For now, return dummy data (the important part is the UI structure)
    return {
      cpu: 0,
      memory: 0,
    };
  }

  exit() {
    if (this.pollInterval) {
      clearInterval(this.pollInterval);
    }
    if (this.screen) {
      this.screen.destroy();
    }
    process.exit(0);
  }
}

export default TaskLogsTUI;
