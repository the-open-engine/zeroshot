import chalk from 'chalk';
import { loadSchedules } from '../store.js';
import { getDaemonStatus } from '../scheduler.js';

export function listSchedules(_options = {}) {
  const schedules = loadSchedules();
  const scheduleList = Object.values(schedules);

  // Check daemon status
  const daemonStatus = getDaemonStatus();

  if (scheduleList.length === 0) {
    printNoSchedules();
    return;
  }

  printDaemonStatus(daemonStatus);

  // Sort by next run time
  sortSchedulesByNextRun(scheduleList);

  printScheduleHeader();

  for (const schedule of scheduleList) {
    printSchedule(schedule);
  }

  console.log(chalk.dim(`Total: ${scheduleList.length} schedule(s)`));
}

function printNoSchedules() {
  console.log(chalk.dim('No schedules found.'));
  console.log(chalk.dim('\nCreate a schedule:'));
  console.log(chalk.dim('  zeroshot schedule "your prompt" --every 1h'));
  console.log(chalk.dim('  zeroshot schedule "your prompt" --cron "0 * * * *"'));
}

function printDaemonStatus(daemonStatus) {
  if (daemonStatus.running) {
    console.log(chalk.green(`Scheduler: running (PID: ${daemonStatus.pid})`));
  } else if (daemonStatus.stale) {
    console.log(chalk.yellow('Scheduler: not running (stale PID file)'));
  } else {
    console.log(chalk.red('Scheduler: not running'));
    console.log(chalk.dim('  Run: zeroshot scheduler start'));
  }
  console.log();
}

function sortSchedulesByNextRun(scheduleList) {
  scheduleList.sort((a, b) => {
    if (!a.nextRunAt) return 1;
    if (!b.nextRunAt) return -1;
    return new Date(a.nextRunAt) - new Date(b.nextRunAt);
  });
}

function printScheduleHeader() {
  console.log(chalk.bold('Scheduled Tasks:'));
  console.log();
}

function printSchedule(schedule) {
  printScheduleHeaderLine(schedule);
  printSchedulePrompt(schedule);
  printScheduleInterval(schedule);
  printScheduleCron(schedule);
  printScheduleNextRun(schedule);
  printScheduleLastRun(schedule);
  printScheduleVerify(schedule);
  console.log();
}

function printScheduleHeaderLine(schedule) {
  const statusColor = schedule.enabled ? chalk.green : chalk.red;
  const statusText = schedule.enabled ? '●' : '○';
  console.log(`${statusColor(statusText)} ${chalk.cyan(schedule.id)}`);
}

function printSchedulePrompt(schedule) {
  console.log(chalk.dim(`  Prompt: ${schedule.prompt}`));
}

function printScheduleInterval(schedule) {
  if (!schedule.interval) {
    return;
  }
  const human = formatInterval(schedule.interval);
  console.log(chalk.dim(`  Interval: every ${human}`));
}

function printScheduleCron(schedule) {
  if (schedule.cron) {
    console.log(chalk.dim(`  Cron: ${schedule.cron}`));
  }
}

function printScheduleNextRun(schedule) {
  if (!schedule.nextRunAt) {
    return;
  }
  const nextRun = new Date(schedule.nextRunAt);
  const timeUntil = formatTimeUntil(nextRun);
  console.log(chalk.dim(`  Next run: ${nextRun.toISOString()} (${timeUntil})`));
}

function printScheduleLastRun(schedule) {
  if (!schedule.lastRunAt) {
    return;
  }
  console.log(chalk.dim(`  Last run: ${schedule.lastRunAt}`));
  if (schedule.lastTaskId) {
    console.log(chalk.dim(`  Last task: ${schedule.lastTaskId}`));
  }
}

function printScheduleVerify(schedule) {
  if (schedule.verify) {
    console.log(chalk.dim(`  Verify: enabled`));
  }
}

function formatInterval(ms) {
  if (ms >= 7 * 24 * 60 * 60 * 1000) return `${ms / (7 * 24 * 60 * 60 * 1000)}w`;
  if (ms >= 24 * 60 * 60 * 1000) return `${ms / (24 * 60 * 60 * 1000)}d`;
  if (ms >= 60 * 60 * 1000) return `${ms / (60 * 60 * 1000)}h`;
  if (ms >= 60 * 1000) return `${ms / (60 * 1000)}m`;
  return `${ms / 1000}s`;
}

function formatTimeUntil(nextRun) {
  const now = new Date();
  const diff = nextRun - now;

  if (diff < 0) {
    return 'overdue';
  }
  if (diff < 60000) {
    return 'in < 1m';
  }
  if (diff < 3600000) {
    return `in ${Math.round(diff / 60000)}m`;
  }
  if (diff < 86400000) {
    return `in ${Math.round(diff / 3600000)}h`;
  }
  return `in ${Math.round(diff / 86400000)}d`;
}
