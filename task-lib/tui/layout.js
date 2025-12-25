/**
 * TUI Layout for Task Logs
 * Creates a blessed layout for monitoring a single task
 *
 * Layout:
 * - Top: Task info box (ID, status, runtime, CPU, memory)
 * - Middle: Live logs (scrollable, auto-scroll to bottom)
 * - Bottom: Help bar (keyboard shortcuts)
 */

import blessed from 'blessed';
import contrib from 'blessed-contrib';

/**
 * Create task logs TUI layout
 * @param {blessed.screen} screen - Blessed screen instance
 * @param {string} taskId - Task ID being monitored
 * @returns {object} Layout widgets
 */
function createLayout(screen, taskId) {
  // Create 20x12 grid for responsive layout
  const grid = new contrib.grid({ rows: 20, cols: 12, screen });

  // ============================================================
  // TASK INFO BOX (3 rows x 12 cols)
  // Shows: Task ID, Status, Runtime, CPU, Memory
  // ============================================================

  const taskInfoBox = grid.set(0, 0, 3, 12, blessed.box, {
    label: ` Task: ${taskId} `,
    content: '',
    tags: true,
    border: { type: 'line', fg: 'cyan' },
    style: {
      border: { fg: 'cyan' },
      label: { fg: 'cyan' },
    },
    padding: {
      left: 2,
      right: 2,
    },
  });

  // ============================================================
  // LIVE LOGS (15 rows x 12 cols)
  // Scrollable log output with auto-scroll
  // ============================================================

  const logsBox = grid.set(3, 0, 15, 12, contrib.log, {
    fg: 'white',
    label: ' Live Logs ',
    border: { type: 'line', fg: 'cyan' },
    tags: true,
    style: {
      border: { fg: 'cyan' },
      label: { fg: 'cyan' },
      text: { fg: 'white' },
    },
    scrollable: true,
    mouse: true,
    keyable: true,
    alwaysScroll: true,
    scrollbar: {
      ch: ' ',
      track: {
        bg: 'gray',
      },
      style: {
        inverse: true,
      },
    },
  });

  // ============================================================
  // HELP BAR (2 rows x 12 cols)
  // Keyboard shortcuts
  // ============================================================

  const helpBar = grid.set(18, 0, 2, 12, blessed.box, {
    label: ' Help ',
    content:
      '{cyan-fg}[↑/↓]{/} Scroll  ' +
      '{cyan-fg}[PgUp/PgDn]{/} Page  ' +
      '{cyan-fg}[Home/End]{/} Top/Bottom  ' +
      '{cyan-fg}[q]{/} Quit',
    tags: true,
    border: { type: 'line', fg: 'cyan' },
    style: {
      border: { fg: 'cyan' },
      label: { fg: 'cyan' },
      text: { fg: 'white' },
    },
    padding: {
      left: 1,
      right: 1,
    },
  });

  // Focus on logs by default
  logsBox.focus();

  return {
    screen,
    grid,
    taskInfoBox,
    logsBox,
    helpBar,
  };
}

export { createLayout };
