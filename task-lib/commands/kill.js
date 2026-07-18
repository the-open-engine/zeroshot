import chalk from 'chalk';
import { getTask, updateTask } from '../store.js';
import { isProcessRunning, terminateProcess } from '../runner.js';

export async function killTaskCommand(taskId, options = {}) {
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
    updateTask(taskId, {
      status: 'stale',
      pid: null,
      error: 'Process died unexpectedly',
    });
    return;
  }

  const result = await terminateProcess(task.pid, options);

  if (result.terminated) {
    const suffix = result.escalated ? ' after SIGKILL escalation' : ' with SIGTERM';
    console.log(chalk.green(`✓ Killed task ${taskId} (PID: ${task.pid})${suffix}`));
    updateTask(taskId, {
      status: 'killed',
      pid: null,
      exitCode: result.escalated ? 137 : 143,
      error: result.escalated ? 'Killed by user after SIGKILL escalation' : 'Killed by user',
    });
  } else {
    console.log(chalk.red(`Failed to kill task ${taskId}`));
    updateTask(taskId, {
      status: 'failed',
      pid: null,
      error: result.error || 'Process termination failed',
    });
  }
}
