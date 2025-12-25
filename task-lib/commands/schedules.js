import chalk from 'chalk';
import { loadSchedules } from '../store.js';
import { getDaemonStatus } from '../scheduler.js';

export function listSchedules(_options = {}) {
  const schedules = loadSchedules();
  const scheduleList = Object.values(schedules);

  // Check daemon status
  const daemonStatus = getDaemonStatus();

  if (scheduleList.length === 0) {
    console.log(chalk.dim('No schedules found.'));
    console.log(chalk.dim('\nCreate a schedule:'));
    console.log(chalk.dim('  zeroshot schedule "your prompt" --every 1h'));
    console.log(chalk.dim('  zeroshot schedule "your prompt" --cron "0 * * * *"'));
    return;
  }

  // Show daemon status
  if (daemonStatus.running) {
    console.log(chalk.green(`Scheduler: running (PID: ${daemonStatus.pid})`));
  } else if (daemonStatus.stale) {
    console.log(chalk.yellow('Scheduler: not running (stale PID file)'));
  } else {
    console.log(chalk.red('Scheduler: not running'));
    console.log(chalk.dim('  Run: zeroshot scheduler start'));
  }
  console.log();

  // Sort by next run time
  scheduleList.sort((a, b) => {
    if (!a.nextRunAt) return 1;
    if (!b.nextRunAt) return -1;
    return new Date(a.nextRunAt) - new Date(b.nextRunAt);
  });

  console.log(chalk.bold('Scheduled Tasks:'));
  console.log();

  for (const schedule of scheduleList) {
    const statusColor = schedule.enabled ? chalk.green : chalk.red;
    const statusText = schedule.enabled ? '●' : '○';

    console.log(`${statusColor(statusText)} ${chalk.cyan(schedule.id)}`);
    console.log(chalk.dim(`  Prompt: ${schedule.prompt}`));

    if (schedule.interval) {
      const ms = schedule.interval;
      let human;
      if (ms >= 7 * 24 * 60 * 60 * 1000) human = `${ms / (7 * 24 * 60 * 60 * 1000)}w`;
      else if (ms >= 24 * 60 * 60 * 1000) human = `${ms / (24 * 60 * 60 * 1000)}d`;
      else if (ms >= 60 * 60 * 1000) human = `${ms / (60 * 60 * 1000)}h`;
      else if (ms >= 60 * 1000) human = `${ms / (60 * 1000)}m`;
      else human = `${ms / 1000}s`;
      console.log(chalk.dim(`  Interval: every ${human}`));
    }
    if (schedule.cron) {
      console.log(chalk.dim(`  Cron: ${schedule.cron}`));
    }

    if (schedule.nextRunAt) {
      const nextRun = new Date(schedule.nextRunAt);
      const now = new Date();
      const diff = nextRun - now;

      let timeUntil;
      if (diff < 0) {
        timeUntil = 'overdue';
      } else if (diff < 60000) {
        timeUntil = 'in < 1m';
      } else if (diff < 3600000) {
        timeUntil = `in ${Math.round(diff / 60000)}m`;
      } else if (diff < 86400000) {
        timeUntil = `in ${Math.round(diff / 3600000)}h`;
      } else {
        timeUntil = `in ${Math.round(diff / 86400000)}d`;
      }

      console.log(chalk.dim(`  Next run: ${nextRun.toISOString()} (${timeUntil})`));
    }

    if (schedule.lastRunAt) {
      console.log(chalk.dim(`  Last run: ${schedule.lastRunAt}`));
      if (schedule.lastTaskId) {
        console.log(chalk.dim(`  Last task: ${schedule.lastTaskId}`));
      }
    }

    if (schedule.verify) {
      console.log(chalk.dim(`  Verify: enabled`));
    }

    console.log();
  }

  console.log(chalk.dim(`Total: ${scheduleList.length} schedule(s)`));
}
