import chalk from 'chalk';
import { getTask, updateTask } from '../store.js';
import { getTaskRuntimeState, reconcileTasks, terminateTask } from '../runner.js';

export async function killTaskCommand(taskId) {
  await reconcileTasks();
  const task = getTask(taskId);

  if (!task) {
    console.log(chalk.red(`Task not found: ${taskId}`));
    process.exit(1);
  }

  if (task.status !== 'running') {
    console.log(chalk.yellow(`Task is not running (status: ${task.status})`));
    return;
  }

  const runtime = getTaskRuntimeState(task);
  if (!runtime.running) {
    console.log(chalk.yellow('Process already dead, updating status...'));
    updateTask(taskId, {
      status: 'stale',
      pid: null,
      watcherPid: null,
      error: 'Process died unexpectedly',
    });
    return;
  }

  const killed = await terminateTask(task);

  if (killed.signaled && killed.exited) {
    console.log(chalk.green(`✓ Stopped task ${taskId}`));
    updateTask(taskId, {
      status: 'killed',
      pid: null,
      watcherPid: null,
      error: 'Killed by user',
    });
  } else {
    console.log(chalk.red(`Failed to kill task ${taskId}`));
  }
}
