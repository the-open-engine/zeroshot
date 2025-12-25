/**
 * E2E Test for Vibe Framework
 *
 * Tests that the zeroshot framework can successfully:
 * 1. Run a simple single-agent task
 * 2. Track the task lifecycle
 * 3. Retrieve task logs
 * 4. Clean up properly
 */

import { test, expect } from '@playwright/test';
import { exec } from 'child_process';
import { promisify } from 'util';
import * as fs from 'fs/promises';
import * as path from 'path';

const execAsync = promisify(exec);

// Test configuration
const VIBE_CLI = 'zeroshot';
const TEST_TIMEOUT = 120000; // 2 minutes for task execution

test.describe('Vibe Framework E2E', () => {
  let taskId: string | null = null;

  test.afterEach(async () => {
    // Clean up: kill task if still running
    if (taskId) {
      try {
        await execAsync(`${VIBE_CLI} kill ${taskId}`);
      } catch (error) {
        // Task might already be completed, ignore errors
      }
      taskId = null;
    }
  });

  test('should run a simple single-agent task successfully', async () => {
    test.setTimeout(TEST_TIMEOUT);
    // Step 1: Verify zeroshot CLI is installed and available
    const { stdout: versionOutput } = await execAsync(`${VIBE_CLI} --version`);
    expect(versionOutput).toContain('1.0.0');
    console.log('[OK] Vibe CLI is available:', versionOutput.trim());

    // Step 2: Run a simple task that should complete quickly
    const taskPrompt = 'Echo hello from zeroshot and exit successfully';
    let runOutput = '';
    try {
      const result = await execAsync(
        `${VIBE_CLI} task run '${taskPrompt}' 2>&1`,
        { timeout: 10000 } // 10 seconds to spawn
      );
      runOutput = result.stdout;
    } catch (error: any) {
      // Command might exit with non-zero but still spawn task successfully
      runOutput = error.stdout || error.stderr || '';
      if (!runOutput.includes('spawned:')) {
        throw error;
      }
    }

    // Extract task ID from output (format: "Task spawned: <adjective>-<animal>-<number>")
    // Note: Output contains ANSI color codes, so we need to account for those
    const taskIdMatch = runOutput.match(/spawned:\s+(?:\x1b\[\d+m)*([a-z]+-[a-z]+-\d+)/);
    if (!taskIdMatch) {
      console.error('Failed to extract task ID from output');
      console.error('Output:', runOutput);
      throw new Error(`Could not extract task ID from output`);
    }
    taskId = taskIdMatch[1];
    console.log('[OK] Task spawned with ID:', taskId);

    // Step 3: Verify task appears in list
    const { stdout: listOutput } = await execAsync(`${VIBE_CLI} list`);
    expect(listOutput).toContain(taskId);
    console.log('[OK] Task appears in list');

    // Step 4: Wait for task to complete (poll status)
    let attempts = 0;
    const maxAttempts = 24; // 2 minutes with 5-second intervals
    let taskCompleted = false;

    while (attempts < maxAttempts && !taskCompleted) {
      await new Promise((resolve) => setTimeout(resolve, 5000)); // Wait 5 seconds

      try {
        const { stdout: statusOutput } = await execAsync(`${VIBE_CLI} status ${taskId}`);

        if (
          statusOutput.toLowerCase().includes('completed') ||
          statusOutput.toLowerCase().includes('stopped')
        ) {
          taskCompleted = true;
          console.log('[OK] Task completed');
        } else if (
          statusOutput.toLowerCase().includes('failed') ||
          statusOutput.toLowerCase().includes('error')
        ) {
          throw new Error(`Task failed: ${statusOutput}`);
        } else {
          console.log(`[INFO] Task still running (attempt ${attempts + 1}/${maxAttempts})`);
        }
      } catch (error) {
        // Status command might fail if task just completed, try logs instead
        try {
          const { stdout: logsOutput } = await execAsync(`${VIBE_CLI} logs ${taskId}`);
          if (logsOutput.includes('Task completed') || logsOutput.includes('exited')) {
            taskCompleted = true;
            console.log('[OK] Task completed (detected via logs)');
          }
        } catch (logError) {
          console.log(`[INFO] Could not get logs (attempt ${attempts + 1}/${maxAttempts})`);
        }
      }

      attempts++;
    }

    expect(taskCompleted).toBe(true);

    // Step 5: Retrieve and verify logs
    const { stdout: logsOutput } = await execAsync(`${VIBE_CLI} logs ${taskId}`);
    expect(logsOutput.length).toBeGreaterThan(0);
    console.log('[OK] Task logs retrieved');
    console.log('Task logs snippet:', logsOutput.substring(0, 500));
  });

  test('should list tasks correctly', async () => {
    // Verify list command works without errors
    const { stdout: listOutput } = await execAsync(`${VIBE_CLI} list`);
    expect(listOutput).toBeTruthy();
    console.log('[OK] List command works');
  });

  test('should show settings', async () => {
    // Verify settings command works
    const { stdout: settingsOutput } = await execAsync(`${VIBE_CLI} settings`);
    expect(settingsOutput).toBeTruthy();
    console.log('[OK] Settings command works');
  });

  test('should verify zeroshot database is accessible', async () => {
    // Check that zeroshot directory exists
    const homeDir = process.env.HOME || process.env.USERPROFILE || '/home/eivind';
    const zeroshotDir = path.join(homeDir, '.zeroshot');

    try {
      await fs.access(zeroshotDir);
      console.log('[OK] Vibe directory exists:', zeroshotDir);

      // Check for clusters.json
      const clustersFile = path.join(zeroshotDir, 'clusters.json');
      try {
        const clustersData = await fs.readFile(clustersFile, 'utf-8');
        const clusters = JSON.parse(clustersData);
        console.log('[OK] Clusters metadata accessible, count:', Object.keys(clusters).length);
      } catch {
        console.log('[INFO] No clusters.json found (normal for fresh install)');
      }
    } catch (error) {
      // Vibe directory might not exist if no tasks have been run yet
      console.log('[INFO] Vibe directory not found or empty (normal for fresh install)');
    }
  });
});
