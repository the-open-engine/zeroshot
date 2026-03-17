import { unlinkSync, existsSync } from 'fs';
import chalk from 'chalk';
import { loadTasks, saveTasks } from '../store.js';
import { getTaskRuntimeState, reconcileTasks, terminateTask } from '../runner.js';

export async function cleanTasks(options = {}) {
  await reconcileTasks();
  const tasks = loadTasks();
  const taskList = Object.values(tasks);

  if (taskList.length === 0) {
    console.log(chalk.dim('No tasks to clean.'));
    return;
  }

  const toRemove = [];

  for (const task of taskList) {
    const shouldRemove =
      options.all ||
      (options.completed && task.status === 'completed') ||
      (options.failed &&
        (task.status === 'failed' || task.status === 'stale' || task.status === 'killed'));

    if (shouldRemove) {
      toRemove.push(task);
    }
  }

  if (toRemove.length === 0) {
    console.log(chalk.dim('No tasks match the criteria.'));
    return;
  }

  console.log(chalk.dim(`Removing ${toRemove.length} task(s)...\n`));

  for (const task of toRemove) {
    const runtime = getTaskRuntimeState(task);
    if (runtime.running) {
      const termination = await terminateTask(task);
      if (!(termination.signaled && termination.exited)) {
        console.log(chalk.red(`  Skipped: ${task.id} [process still alive]`));
        continue;
      }
    }

    // Delete log file
    if (task.logFile && existsSync(task.logFile)) {
      unlinkSync(task.logFile);
    }

    // Remove from tasks
    delete tasks[task.id];

    console.log(chalk.dim(`  Removed: ${task.id} [${task.status}]`));
  }

  saveTasks(tasks);

  console.log(chalk.green(`\n✓ Cleaned ${toRemove.length} task(s)`));
}
