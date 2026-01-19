import chalk from 'chalk';
import { getTask } from '../store.js';
import { spawnTask } from '../runner.js';

export async function resumeTask(taskId, newPrompt) {
  const task = getTask(taskId);

  if (!task) {
    console.log(chalk.red(`Task not found: ${taskId}`));
    process.exit(1);
  }

  if (task.status === 'running') {
    console.log(
      chalk.yellow(`Task is still running. Use 'zeroshot logs -f ${taskId}' to follow output.`)
    );
    return;
  }

  const prompt = newPrompt || 'Continue from where you left off. Complete the task.';

  console.log(chalk.dim(`Resuming task ${taskId}...`));
  console.log(chalk.dim(`Original prompt: ${task.prompt}`));
  console.log(chalk.dim(`Resume prompt: ${prompt}`));

  const newTask = await spawnTask(prompt, {
    cwd: task.cwd,
    continue: true, // Use --continue to load most recent session in that directory
    sessionId: task.sessionId,
    provider: task.provider,
  });

  console.log(chalk.green(`\n✓ Resumed as new task: ${chalk.cyan(newTask.id)}`));
  console.log(chalk.dim(`  PID: ${newTask.pid}`));
  console.log(chalk.dim(`  Log: ${newTask.logFile}`));

  console.log(chalk.dim('\nCommands:'));
  console.log(chalk.dim(`  zeroshot logs -f ${newTask.id}   # Follow output`));
  console.log();

  return newTask;
}
