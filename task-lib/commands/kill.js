import chalk from 'chalk';
import { getTask, updateTask } from '../store.js';
import { killTask as killProcess, isProcessRunning } from '../runner.js';

export function killTaskCommand(taskId) {
  const task = getTask(taskId);

  if (!task) {
    console.log(chalk.red(`Task not found: ${taskId}`));
    process.exit(1);
  }

  if (task.status !== 'running') {
    console.log(chalk.yellow(`Task is not running (status: ${task.status})`));
    return;
  }

  if (!isProcessRunning(task.pid)) {
    console.log(chalk.yellow('Process already dead, updating status...'));
    updateTask(taskId, { status: 'stale', error: 'Process died unexpectedly' });
    return;
  }

  const killed = killProcess(task.pid);

  if (killed) {
    console.log(chalk.green(`✓ Sent SIGTERM to task ${taskId} (PID: ${task.pid})`));
    updateTask(taskId, { status: 'killed', error: 'Killed by user' });
  } else {
    console.log(chalk.red(`Failed to kill task ${taskId}`));
  }
}
