/**
 * Keybindings for TUI
 *
 * Handles:
 * - Navigation (up/down, j/k)
 * - Actions (kill, stop, export, logs)
 * - Confirmations for destructive actions
 */

const blessed = require('blessed');
const { spawn } = require('child_process');

function setupKeybindings(screen, widgets, tui, orchestrator) {
  // Enter - drill into cluster detail view
  screen.key(['enter'], () => {
    if (tui.viewMode === 'overview' && tui.clusters.length > 0) {
      const selectedCluster = tui.clusters[tui.selectedIndex];
      if (selectedCluster) {
        tui.viewMode = 'detail';
        tui.detailClusterId = selectedCluster.id;
        tui.renderer.setSelectedCluster(selectedCluster.id);
        tui.messages = []; // Clear old messages

        // Update help text
        widgets.helpBar.setContent(
          '{cyan-fg}[Esc]{/} Back  {cyan-fg}[k]{/} Kill  {cyan-fg}[s]{/} Stop  {cyan-fg}[e]{/} Export  {cyan-fg}[l]{/} Logs  {cyan-fg}[r]{/} Refresh  {cyan-fg}[q]{/} Quit'
        );

        // Switch to detail layout: hide clusters/stats, show agents/logs
        widgets.clustersTable.hide();
        widgets.statsBox.hide();
        widgets.agentTable.show();
        widgets.logsBox.show();
        screen.render();
      }
    }
  });

  // Escape - back to overview
  screen.key(['escape'], () => {
    if (tui.viewMode === 'detail') {
      tui.viewMode = 'overview';
      tui.detailClusterId = null;
      tui.renderer.setSelectedCluster(null);
      tui.messages = []; // Clear messages

      // Update help text
      widgets.helpBar.setContent(
        '{cyan-fg}[Enter]{/} View  {cyan-fg}[↑/↓]{/} Navigate  {cyan-fg}[k]{/} Kill  {cyan-fg}[s]{/} Stop  {cyan-fg}[l]{/} Logs  {cyan-fg}[r]{/} Refresh  {cyan-fg}[q]{/} Quit'
      );

      // Switch to overview layout: show clusters/stats, hide agents/logs
      widgets.clustersTable.show();
      widgets.statsBox.show();
      widgets.agentTable.hide();
      widgets.logsBox.hide();
      screen.render();
    }
  });

  // Navigation - up
  screen.key(['up', 'k'], () => {
    if (tui.clusters.length === 0) return;
    tui.selectedIndex = Math.max(0, tui.selectedIndex - 1);
    tui.renderer.renderClustersTable(tui.clusters, tui.selectedIndex);

    // Update agent table and logs for newly selected cluster
    const selectedCluster = tui.clusters[tui.selectedIndex];
    if (selectedCluster) {
      // CRITICAL: Tell renderer which cluster is selected
      tui.renderer.setSelectedCluster(selectedCluster.id);

      // Clear old messages from previous cluster
      tui.messages = [];

      const status = orchestrator.getStatus(selectedCluster.id);
      tui.renderer.renderAgentTable(status.agents, tui.resourceStats);
    }

    screen.render();
  });

  // Navigation - down
  screen.key(['down', 'j'], () => {
    if (tui.clusters.length === 0) return;
    tui.selectedIndex = Math.min(tui.clusters.length - 1, tui.selectedIndex + 1);
    tui.renderer.renderClustersTable(tui.clusters, tui.selectedIndex);

    // Update agent table and logs for newly selected cluster
    const selectedCluster = tui.clusters[tui.selectedIndex];
    if (selectedCluster) {
      // CRITICAL: Tell renderer which cluster is selected
      tui.renderer.setSelectedCluster(selectedCluster.id);

      // Clear old messages from previous cluster
      tui.messages = [];

      const status = orchestrator.getStatus(selectedCluster.id);
      tui.renderer.renderAgentTable(status.agents, tui.resourceStats);
    }

    screen.render();
  });

  // Kill selected cluster (with confirmation)
  screen.key(['K'], () => {
    if (tui.clusters.length === 0) return;
    const selectedCluster = tui.clusters[tui.selectedIndex];
    if (!selectedCluster) return;

    // Create confirmation dialog
    const question = blessed.question({
      parent: screen,
      border: 'line',
      height: 'shrink',
      width: 'half',
      top: 'center',
      left: 'center',
      label: ' {bold}{red-fg}Confirm Kill{/red-fg}{/bold} ',
      tags: true,
      keys: true,
      vi: true,
    });

    question.ask(
      `Kill cluster ${selectedCluster.id}?\n\n(This will force-stop all agents)`,
      async (err, value) => {
        if (err) return;
        if (value) {
          try {
            await orchestrator.kill(selectedCluster.id);
            tui.messages.push({
              timestamp: new Date().toISOString(),
              text: `✓ Killed cluster ${selectedCluster.id}`,
              level: 'success',
            });
            tui.renderer.renderLogs(tui.messages.slice(-20));
          } catch (error) {
            tui.messages.push({
              timestamp: new Date().toISOString(),
              text: `✗ Failed to kill cluster: ${error.message}`,
              level: 'error',
            });
            tui.renderer.renderLogs(tui.messages.slice(-20));
          }
          screen.render();
        }
      }
    );
  });

  // Stop selected cluster (with confirmation)
  screen.key(['s'], () => {
    if (tui.clusters.length === 0) return;
    const selectedCluster = tui.clusters[tui.selectedIndex];
    if (!selectedCluster) return;

    // Create confirmation dialog
    const question = blessed.question({
      parent: screen,
      border: 'line',
      height: 'shrink',
      width: 'half',
      top: 'center',
      left: 'center',
      label: ' {bold}{yellow-fg}Confirm Stop{/yellow-fg}{/bold} ',
      tags: true,
      keys: true,
      vi: true,
    });

    question.ask(
      `Stop cluster ${selectedCluster.id}?\n\n(This will gracefully stop all agents)`,
      async (err, value) => {
        if (err) return;
        if (value) {
          try {
            await orchestrator.stop(selectedCluster.id);
            tui.messages.push({
              timestamp: new Date().toISOString(),
              text: `✓ Stopped cluster ${selectedCluster.id}`,
              level: 'success',
            });
            tui.renderer.renderLogs(tui.messages.slice(-20));
          } catch (error) {
            tui.messages.push({
              timestamp: new Date().toISOString(),
              text: `✗ Failed to stop cluster: ${error.message}`,
              level: 'error',
            });
            tui.renderer.renderLogs(tui.messages.slice(-20));
          }
          screen.render();
        }
      }
    );
  });

  // Export selected cluster
  screen.key(['e'], () => {
    if (tui.clusters.length === 0) return;
    const selectedCluster = tui.clusters[tui.selectedIndex];
    if (!selectedCluster) return;

    try {
      const markdown = orchestrator.export(selectedCluster.id, 'markdown');
      const fs = require('fs');
      const filename = `${selectedCluster.id}-export.md`;
      fs.writeFileSync(filename, markdown);

      tui.messages.push({
        timestamp: new Date().toISOString(),
        text: `✓ Exported cluster to ${filename}`,
        level: 'success',
      });
      tui.renderer.renderLogs(tui.messages.slice(-20));
      screen.render();
    } catch (error) {
      tui.messages.push({
        timestamp: new Date().toISOString(),
        text: `✗ Failed to export cluster: ${error.message}`,
        level: 'error',
      });
      tui.renderer.renderLogs(tui.messages.slice(-20));
      screen.render();
    }
  });

  // Show full logs (spawn zeroshot logs -f in new terminal)
  screen.key(['l'], () => {
    if (tui.clusters.length === 0) return;
    const selectedCluster = tui.clusters[tui.selectedIndex];
    if (!selectedCluster) return;

    try {
      // Detect terminal emulator
      const term =
        process.env.TERM_PROGRAM || (process.env.COLORTERM ? 'gnome-terminal' : null) || 'xterm';

      let cmd, args;
      if (term === 'iTerm.app' || term === 'Apple_Terminal') {
        // macOS
        cmd = 'osascript';
        args = [
          '-e',
          `tell application "Terminal" to do script "zeroshot logs ${selectedCluster.id} -f"`,
        ];
      } else {
        // Linux - try common terminal emulators
        const terminals = ['gnome-terminal', 'konsole', 'xterm', 'urxvt', 'alacritty', 'kitty'];
        cmd =
          terminals.find((t) => {
            try {
              require('child_process').execSync(`which ${t}`, {
                stdio: 'ignore',
              });
              return true;
            } catch {
              return false;
            }
          }) || 'xterm';

        if (cmd === 'gnome-terminal' || cmd === 'konsole') {
          args = [
            '--',
            'bash',
            '-c',
            `zeroshot logs ${selectedCluster.id} -f; read -p "Press enter to close..."`,
          ];
        } else {
          args = [
            '-e',
            'bash',
            '-c',
            `zeroshot logs ${selectedCluster.id} -f; read -p "Press enter to close..."`,
          ];
        }
      }

      spawn(cmd, args, { detached: true, stdio: 'ignore' });

      tui.messages.push({
        timestamp: new Date().toISOString(),
        text: `✓ Opened logs for ${selectedCluster.id} in new terminal`,
        level: 'success',
      });
      tui.renderer.renderLogs(tui.messages.slice(-20));
      screen.render();
    } catch (error) {
      tui.messages.push({
        timestamp: new Date().toISOString(),
        text: `✗ Failed to open logs: ${error.message}`,
        level: 'error',
      });
      tui.renderer.renderLogs(tui.messages.slice(-20));
      screen.render();
    }
  });

  // Force refresh
  screen.key(['r'], () => {
    tui.messages.push({
      timestamp: new Date().toISOString(),
      text: '↻ Refreshing...',
      level: 'info',
    });
    tui.renderer.renderLogs(tui.messages.slice(-20));
    screen.render();

    // Trigger immediate poll
    if (tui.poller) {
      tui.poller.poll();
    }
  });

  // Exit (with confirmation)
  screen.key(['q', 'C-c'], () => {
    const question = blessed.question({
      parent: screen,
      border: 'line',
      height: 'shrink',
      width: 'half',
      top: 'center',
      left: 'center',
      label: ' {bold}Confirm Exit{/bold} ',
      tags: true,
      keys: true,
      vi: true,
    });

    question.ask('Exit TUI?\n\n(Clusters will continue running)', (err, value) => {
      if (err) return;
      if (value) {
        tui.exit();
      }
    });
  });

  // Help
  screen.key(['?', 'h'], () => {
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
  });
}

module.exports = { setupKeybindings };
