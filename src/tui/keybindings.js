/**
 * Keybindings for TUI
 *
 * Handles:
 * - Navigation (up/down, j/k)
 * - Actions (kill, stop, export, logs)
 * - Confirmations for destructive actions
 */

const blessed = require('blessed');
const fs = require('fs');
const { spawn } = require('child_process');
const { execSync } = require('../lib/safe-exec'); // Enforces timeouts

const HELP_TEXT_DETAIL =
  '{cyan-fg}[Esc]{/} Back  {cyan-fg}[k]{/} Kill  {cyan-fg}[s]{/} Stop  {cyan-fg}[e]{/} Export  {cyan-fg}[l]{/} Logs  {cyan-fg}[r]{/} Refresh  {cyan-fg}[q]{/} Quit';
const HELP_TEXT_OVERVIEW =
  '{cyan-fg}[Enter]{/} View  {cyan-fg}[↑/↓]{/} Navigate  {cyan-fg}[k]{/} Kill  {cyan-fg}[s]{/} Stop  {cyan-fg}[l]{/} Logs  {cyan-fg}[r]{/} Refresh  {cyan-fg}[q]{/} Quit';

function getSelectedCluster(tui) {
  if (tui.clusters.length === 0) {
    return null;
  }

  return tui.clusters[tui.selectedIndex] || null;
}

function pushLogMessage(tui, text, level) {
  tui.messages.push({
    timestamp: new Date().toISOString(),
    text,
    level,
  });
  tui.renderer.renderLogs(tui.messages.slice(-20));
}

function enterDetailView(screen, widgets, tui) {
  if (tui.viewMode !== 'overview') {
    return;
  }

  const selectedCluster = getSelectedCluster(tui);
  if (!selectedCluster) {
    return;
  }

  tui.viewMode = 'detail';
  tui.detailClusterId = selectedCluster.id;
  tui.renderer.setSelectedCluster(selectedCluster.id);
  tui.messages = [];

  widgets.helpBar.setContent(HELP_TEXT_DETAIL);
  widgets.clustersTable.hide();
  widgets.statsBox.hide();
  widgets.agentTable.show();
  widgets.logsBox.show();
  screen.render();
}

function exitDetailView(screen, widgets, tui) {
  if (tui.viewMode !== 'detail') {
    return;
  }

  tui.viewMode = 'overview';
  tui.detailClusterId = null;
  tui.renderer.setSelectedCluster(null);
  tui.messages = [];

  widgets.helpBar.setContent(HELP_TEXT_OVERVIEW);
  widgets.clustersTable.show();
  widgets.statsBox.show();
  widgets.agentTable.hide();
  widgets.logsBox.hide();
  screen.render();
}

function moveSelection(screen, tui, orchestrator, delta) {
  if (tui.clusters.length === 0) {
    return;
  }

  tui.selectedIndex = Math.min(tui.clusters.length - 1, Math.max(0, tui.selectedIndex + delta));
  tui.renderer.renderClustersTable(tui.clusters, tui.selectedIndex);

  const selectedCluster = tui.clusters[tui.selectedIndex];
  if (selectedCluster) {
    tui.renderer.setSelectedCluster(selectedCluster.id);
    tui.messages = [];

    const status = orchestrator.getStatus(selectedCluster.id);
    tui.renderer.renderAgentTable(status.agents, tui.resourceStats);
  }

  screen.render();
}

function createConfirmationDialog(screen, label, color) {
  const labelText = color
    ? ` {bold}{${color}-fg}${label}{/${color}-fg}{/bold} `
    : ` {bold}${label}{/bold} `;

  return blessed.question({
    parent: screen,
    border: 'line',
    height: 'shrink',
    width: 'half',
    top: 'center',
    left: 'center',
    label: labelText,
    tags: true,
    keys: true,
    vi: true,
  });
}

function confirmClusterAction(options) {
  const { screen, tui, selectedCluster, label, color, prompt, action, successText, failureText } =
    options;
  const question = createConfirmationDialog(screen, label, color);

  question.ask(prompt, async (err, value) => {
    if (err || !value) {
      return;
    }

    try {
      await action(selectedCluster);
      pushLogMessage(tui, successText(selectedCluster), 'success');
    } catch (error) {
      pushLogMessage(tui, failureText(error), 'error');
    }

    screen.render();
  });
}

function handleKillCluster(screen, tui, orchestrator) {
  const selectedCluster = getSelectedCluster(tui);
  if (!selectedCluster) {
    return;
  }

  confirmClusterAction({
    screen,
    tui,
    selectedCluster,
    label: 'Confirm Kill',
    color: 'red',
    prompt: `Kill cluster ${selectedCluster.id}?\n\n(This will force-stop all agents)`,
    action: (cluster) => orchestrator.kill(cluster.id),
    successText: (cluster) => `✓ Killed cluster ${cluster.id}`,
    failureText: (error) => `✗ Failed to kill cluster: ${error.message}`,
  });
}

function handleStopCluster(screen, tui, orchestrator) {
  const selectedCluster = getSelectedCluster(tui);
  if (!selectedCluster) {
    return;
  }

  confirmClusterAction({
    screen,
    tui,
    selectedCluster,
    label: 'Confirm Stop',
    color: 'yellow',
    prompt: `Stop cluster ${selectedCluster.id}?\n\n(This will gracefully stop all agents)`,
    action: (cluster) => orchestrator.stop(cluster.id),
    successText: (cluster) => `✓ Stopped cluster ${cluster.id}`,
    failureText: (error) => `✗ Failed to stop cluster: ${error.message}`,
  });
}

function handleExportCluster(screen, tui, orchestrator) {
  const selectedCluster = getSelectedCluster(tui);
  if (!selectedCluster) {
    return;
  }

  try {
    const markdown = orchestrator.export(selectedCluster.id, 'markdown');
    const filename = `${selectedCluster.id}-export.md`;
    fs.writeFileSync(filename, markdown);
    pushLogMessage(tui, `✓ Exported cluster to ${filename}`, 'success');
  } catch (error) {
    pushLogMessage(tui, `✗ Failed to export cluster: ${error.message}`, 'error');
  }

  screen.render();
}

function findTerminalCommand() {
  const terminals = ['gnome-terminal', 'konsole', 'xterm', 'urxvt', 'alacritty', 'kitty'];

  for (const terminal of terminals) {
    try {
      execSync(`which ${terminal}`, { stdio: 'ignore' });
      return terminal;
    } catch {
      // Ignore missing terminal
    }
  }

  return 'xterm';
}

function buildLogCommand(clusterId) {
  const term =
    process.env.TERM_PROGRAM || (process.env.COLORTERM ? 'gnome-terminal' : null) || 'xterm';

  if (term === 'iTerm.app' || term === 'Apple_Terminal') {
    return {
      cmd: 'osascript',
      args: ['-e', `tell application "Terminal" to do script "zeroshot logs ${clusterId} -f"`],
    };
  }

  const cmd = findTerminalCommand();
  const logCommand = `zeroshot logs ${clusterId} -f; read -p "Press enter to close..."`;

  if (cmd === 'gnome-terminal' || cmd === 'konsole') {
    return { cmd, args: ['--', 'bash', '-c', logCommand] };
  }

  return { cmd, args: ['-e', 'bash', '-c', logCommand] };
}

function handleOpenLogs(screen, tui) {
  const selectedCluster = getSelectedCluster(tui);
  if (!selectedCluster) {
    return;
  }

  try {
    const { cmd, args } = buildLogCommand(selectedCluster.id);
    spawn(cmd, args, { detached: true, stdio: 'ignore' });
    pushLogMessage(tui, `✓ Opened logs for ${selectedCluster.id} in new terminal`, 'success');
  } catch (error) {
    pushLogMessage(tui, `✗ Failed to open logs: ${error.message}`, 'error');
  }

  screen.render();
}

function handleRefresh(screen, tui) {
  pushLogMessage(tui, '↻ Refreshing...', 'info');
  screen.render();

  if (tui.poller) {
    tui.poller.poll();
  }
}

function handleExit(screen, tui) {
  const question = createConfirmationDialog(screen, 'Confirm Exit');

  question.ask('Exit TUI?\n\n(Clusters will continue running)', (err, value) => {
    if (err || !value) {
      return;
    }

    tui.exit();
  });
}

function handleHelp(screen) {
  const helpBox = blessed.box({
    parent: screen,
    border: 'line',
    height: '80%',
    width: '60%',
    top: 'center',
    left: 'center',
    label: ' {bold}Keybindings{/bold} ',
    tags: true,
    keys: true,
    vi: true,
    scrollable: true,
    alwaysScroll: true,
    content: `
{bold}Navigation:{/bold}
  ↑/k       Move selection up
  ↓/j       Move selection down

{bold}Actions:{/bold}
  K         Kill selected cluster (force stop)
  s         Stop selected cluster (graceful)
  e         Export selected cluster to markdown
  l         Show full logs in new terminal
  r         Force refresh

{bold}Other:{/bold}
  ?/h       Show this help
  q/Ctrl-C  Exit TUI

Press any key to close...
    `.trim(),
  });

  helpBox.key(['escape', 'q', 'enter', 'space'], () => {
    helpBox.destroy();
    screen.render();
  });

  screen.render();
}

function setupKeybindings(screen, widgets, tui, orchestrator) {
  screen.key(['enter'], () => enterDetailView(screen, widgets, tui));
  screen.key(['escape'], () => exitDetailView(screen, widgets, tui));
  screen.key(['up', 'k'], () => moveSelection(screen, tui, orchestrator, -1));
  screen.key(['down', 'j'], () => moveSelection(screen, tui, orchestrator, 1));
  screen.key(['K'], () => handleKillCluster(screen, tui, orchestrator));
  screen.key(['s'], () => handleStopCluster(screen, tui, orchestrator));
  screen.key(['e'], () => handleExportCluster(screen, tui, orchestrator));
  screen.key(['l'], () => handleOpenLogs(screen, tui));
  screen.key(['r'], () => handleRefresh(screen, tui));
  screen.key(['q', 'C-c'], () => handleExit(screen, tui));
  screen.key(['?', 'h'], () => handleHelp(screen));
}

module.exports = { setupKeybindings };
