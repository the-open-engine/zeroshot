import { getTask } from '../store.js';

/**
 * Get log file path for a task (machine-readable output)
 * Used by cluster/agent-wrapper.js to follow task logs
 * @param {string} taskId - Task ID
 */
export function getLogPath(taskId) {
  const task = getTask(taskId);

  if (!task) {
    console.error(`Task not found: ${taskId}`);
    process.exit(1);
  }

  if (!task.logFile) {
    console.error(`No log file for task: ${taskId}`);
    process.exit(1);
  }

  // Output just the path (machine-readable)
  console.log(task.logFile);
}
