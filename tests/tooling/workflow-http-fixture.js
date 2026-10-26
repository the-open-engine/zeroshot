'use strict';

const assert = require('node:assert/strict');
const { execFile } = require('node:child_process');
const fs = require('node:fs/promises');
const { createServer } = require('node:http');
const os = require('node:os');
const path = require('node:path');
const yaml = require('js-yaml');

async function runWorkflowStep(t, config) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'zeroshot-workflow-http-'));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  const requests = [];
  const server = createServer(async (request, response) => {
    let body = '';
    for await (const chunk of request) body += chunk;
    const record = { method: request.method, url: request.url, body: body && JSON.parse(body) };
    requests.push(record);
    response.setHeader('Content-Type', 'application/json');
    response.setHeader('Connection', 'close');
    await config.respond(record, response, requests.length - 1);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const root = path.resolve(__dirname, '..', '..');
  const workflow = yaml.load(await fs.readFile(path.join(root, config.workflow), 'utf8'));
  const step = workflow.jobs[config.job].steps.find(({ name }) => name === config.step);
  assert.ok(step?.run, `missing workflow step: ${config.step}`);
  const result = await new Promise((resolve, reject) => {
    execFile(
      'bash',
      ['--noprofile', '--norc', '-c', (config.prefix ?? '') + step.run],
      {
        timeout: 10000,
        env: {
          PATH: process.env.PATH,
          GH_CONFIG_DIR: directory,
          GH_HOST: 'github.localhost',
          GH_REPO: 'fixture/repo',
          GH_TOKEN: 'offline-test-token',
          GH_ENTERPRISE_TOKEN: 'offline-test-token',
          GH_PROMPT_DISABLED: '1',
          GH_NO_UPDATE_NOTIFIER: '1',
          HTTP_PROXY: `http://127.0.0.1:${server.address().port}`,
          NO_PROXY: '',
          GITHUB_REPOSITORY: 'fixture/repo',
          GITHUB_SERVER_URL: 'https://github.localhost',
          GITHUB_STEP_SUMMARY: path.join(directory, 'summary'),
          ...config.environment,
        },
      },
      (error, stdout, stderr) => {
        if (error && !Number.isInteger(error.code)) return reject(error);
        resolve({ code: error?.code ?? 0, stdout, stderr });
      }
    );
  });
  return { ...result, requests };
}

module.exports = { runWorkflowStep };
