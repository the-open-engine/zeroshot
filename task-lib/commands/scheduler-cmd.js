import chalk from 'chalk';
import { fork } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { getDaemonStatus, stopDaemon } from '../scheduler.js';
import { SCHEDULER_LOG } from '../config.js';
import { existsSync, readFileSync } from 'fs';

const __dirname = dirname(fileURLToPath(import.meta.url));

export function schedulerCommand(action) {
  switch (action) {
    case 'start':
      startScheduler();
      break;
    case 'stop':
      stopScheduler();
      break;
    case 'status':
      showStatus();
      break;
    case 'logs':
      showLogs();
      break;
    default:
      console.log(chalk.red(`Unknown action: ${action}`));
      console.log(chalk.dim('Available actions: start, stop, status, logs'));
      process.exit(1);
  }
}

function startScheduler() {
  const status = getDaemonStatus();

  if (status.running) {
    console.log(chalk.yellow(`Scheduler already running (PID: ${status.pid})`));
    return;
  }

  if (status.stale) {
    console.log(chalk.dim('Cleaning up stale PID file...'));
  }

  console.log(chalk.dim('Starting scheduler daemon...'));

  const scheduler = fork(join(__dirname, '..', 'scheduler.js'), [], {
    detached: true,
    stdio: 'ignore',
  });
  scheduler.unref();

  // Give it a moment to start
  setTimeout(() => {
    const newStatus = getDaemonStatus();
    if (newStatus.running) {
      console.log(chalk.green(`✓ Scheduler started (PID: ${newStatus.pid})`));
    } else {
      console.log(chalk.red('Failed to start scheduler. Check logs:'));
      console.log(chalk.dim(`  zeroshot scheduler logs`));
    }
  }, 500);
}

function stopScheduler() {
  const stopped = stopDaemon();
  if (!stopped) {
    process.exit(1);
  }
}

function showStatus() {
  const status = getDaemonStatus();

  if (status.running) {
    console.log(chalk.green(`Scheduler: running`));
    console.log(chalk.dim(`  PID: ${status.pid}`));
    console.log(chalk.dim(`  Log: ${SCHEDULER_LOG}`));
  } else if (status.stale) {
    console.log(chalk.yellow('Scheduler: not running (stale PID file)'));
    console.log(chalk.dim('  Run: zeroshot scheduler start'));
  } else {
    console.log(chalk.red('Scheduler: not running'));
    console.log(chalk.dim('  Run: zeroshot scheduler start'));
  }
}

function showLogs() {
  if (!existsSync(SCHEDULER_LOG)) {
    console.log(chalk.dim('No scheduler logs found.'));
    return;
  }

  const content = readFileSync(SCHEDULER_LOG, 'utf-8');
  const lines = content.split('\n').slice(-50); // Last 50 lines
  console.log(lines.join('\n'));
}
