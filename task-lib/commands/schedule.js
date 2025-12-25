import chalk from 'chalk';
import { addSchedule, generateScheduleId, ensureDirs } from '../store.js';
import { parseInterval, calculateNextRun, getDaemonStatus } from '../scheduler.js';
import { fork } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

export function createSchedule(prompt, options = {}) {
  if (!prompt || prompt.trim().length === 0) {
    console.log(chalk.red('Error: Prompt is required'));
    process.exit(1);
  }

  if (!options.every && !options.cron) {
    console.log(chalk.red('Error: Either --every or --cron is required'));
    console.log(chalk.dim('  Examples:'));
    console.log(chalk.dim('    zeroshot schedule "backup" --every 1d'));
    console.log(chalk.dim('    zeroshot schedule "cleanup" --cron "0 2 * * *"'));
    process.exit(1);
  }

  ensureDirs();

  let interval = null;
  let cron = null;

  if (options.every) {
    interval = parseInterval(options.every);
    if (!interval) {
      console.log(chalk.red(`Error: Invalid interval format "${options.every}"`));
      console.log(chalk.dim('  Valid formats: 30s, 5m, 2h, 1d, 1w'));
      process.exit(1);
    }
  }

  if (options.cron) {
    cron = options.cron;
    // Basic validation
    if (cron.trim().split(/\s+/).length !== 5) {
      console.log(chalk.red(`Error: Invalid cron format "${options.cron}"`));
      console.log(chalk.dim('  Format: minute hour day month weekday'));
      console.log(chalk.dim('  Example: "0 2 * * *" (daily at 2am)'));
      process.exit(1);
    }
  }

  const schedule = {
    id: generateScheduleId(),
    prompt: prompt.slice(0, 200) + (prompt.length > 200 ? '...' : ''),
    fullPrompt: prompt,
    cwd: options.cwd || process.cwd(),
    interval,
    cron,
    nextRunAt: null,
    lastRunAt: null,
    lastTaskId: null,
    enabled: true,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  };

  // Calculate first run time
  const nextRun = calculateNextRun(schedule);
  schedule.nextRunAt = nextRun ? nextRun.toISOString() : null;

  addSchedule(schedule);

  console.log(chalk.green(`\n✓ Schedule created: ${chalk.cyan(schedule.id)}`));
  console.log(chalk.dim(`  Prompt: ${schedule.prompt}`));
  console.log(chalk.dim(`  CWD: ${schedule.cwd}`));

  if (interval) {
    const humanInterval = options.every;
    console.log(chalk.dim(`  Interval: every ${humanInterval}`));
  }
  if (cron) {
    console.log(chalk.dim(`  Cron: ${cron}`));
  }
  console.log(chalk.dim(`  Next run: ${nextRun ? nextRun.toISOString() : 'N/A'}`));

  // Check if scheduler is running, start if not
  const status = getDaemonStatus();
  if (!status.running) {
    console.log(chalk.yellow('\n⚠ Scheduler daemon is not running'));
    console.log(chalk.dim('  Starting scheduler daemon...'));

    // Fork scheduler as detached process
    const scheduler = fork(join(__dirname, '..', 'scheduler.js'), [], {
      detached: true,
      stdio: 'ignore',
    });
    scheduler.unref();

    console.log(chalk.green('  ✓ Scheduler daemon started'));
  }

  console.log(chalk.dim('\nCommands:'));
  console.log(chalk.dim(`  zeroshot schedules              # List all schedules`));
  console.log(chalk.dim(`  zeroshot unschedule ${schedule.id}  # Remove this schedule`));
  console.log();

  return schedule;
}
