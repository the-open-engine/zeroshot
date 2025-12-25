import chalk from 'chalk';
import { getSchedule, removeSchedule } from '../store.js';

export function deleteSchedule(scheduleId) {
  const schedule = getSchedule(scheduleId);

  if (!schedule) {
    console.log(chalk.red(`Schedule not found: ${scheduleId}`));
    process.exit(1);
  }

  removeSchedule(scheduleId);

  console.log(chalk.green(`✓ Schedule removed: ${chalk.cyan(scheduleId)}`));
  console.log(chalk.dim(`  Prompt: ${schedule.prompt}`));
}
