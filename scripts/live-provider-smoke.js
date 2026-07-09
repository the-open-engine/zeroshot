#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const helper = require('../lib/agent-cli-provider');

const SENTINEL = process.env.ZEROSHOT_LIVE_SENTINEL || 'ZEROSHOT_LIVE_SMOKE_OK';
const TIMEOUT_MS = Number(process.env.ZEROSHOT_LIVE_TIMEOUT_MS || 120000);
const DEFAULT_PROMPT = `Reply with exactly this text and nothing else: ${SENTINEL}`;

function fail(message) {
  console.error(message);
  process.exitCode = 1;
}

function usage() {
  return [
    'Live provider smoke requires explicit provider selection.',
    '',
    'Examples:',
    '  ZEROSHOT_LIVE_PROVIDERS=pi npm run test:providers:live',
    '  ZEROSHOT_LIVE_PROVIDERS=copilot npm run test:providers:live',
    '  ZEROSHOT_LIVE_PROVIDERS=gateway \\',
    '    ZEROSHOT_LIVE_GATEWAY_BASE_URL=https://openrouter.ai/api/v1 \\',
    '    ZEROSHOT_LIVE_GATEWAY_API_KEY=... \\',
    '    ZEROSHOT_LIVE_GATEWAY_MODEL=openai/gpt-5.4 \\',
    '    npm run test:providers:live',
    '',
    'This command invokes real provider CLIs or a real gateway endpoint. It is intentionally',
    'not part of the normal CI suite because it can require user auth and paid API calls.',
  ].join('\n');
}

function parseProviders() {
  const raw = process.env.ZEROSHOT_LIVE_PROVIDERS;
  if (!raw) {
    console.error(usage());
    process.exit(2);
  }

  const providers = [];
  for (const item of raw.split(',')) {
    const normalized = helper.normalizeProviderName(item.trim());
    if (!normalized) continue;
    if (!helper.listProviderAdapters().includes(normalized)) {
      throw new Error(
        `Unknown provider "${item}". Valid providers: ${helper.listProviderAdapters().join(', ')}`
      );
    }
    if (!providers.includes(normalized)) providers.push(normalized);
  }

  if (providers.length === 0) {
    throw new Error('ZEROSHOT_LIVE_PROVIDERS did not contain any provider names.');
  }
  return providers;
}

function readJsonEnv(key) {
  const value = process.env[key];
  if (!value) return undefined;
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${key} must be valid JSON: ${error.message}`);
  }
}

function liveGatewayOptions(cwd) {
  const baseUrl = process.env.ZEROSHOT_LIVE_GATEWAY_BASE_URL;
  const apiKey = process.env.ZEROSHOT_LIVE_GATEWAY_API_KEY;
  const model = process.env.ZEROSHOT_LIVE_GATEWAY_MODEL;
  const headers = readJsonEnv('ZEROSHOT_LIVE_GATEWAY_HEADERS_JSON');

  for (const [key, value] of [
    ['ZEROSHOT_LIVE_GATEWAY_BASE_URL', baseUrl],
    ['ZEROSHOT_LIVE_GATEWAY_API_KEY', apiKey],
    ['ZEROSHOT_LIVE_GATEWAY_MODEL', model],
  ]) {
    if (!value) throw new Error(`gateway live smoke requires ${key}.`);
  }

  return {
    cwd,
    gateway: {
      baseUrl,
      apiKey,
      model,
      ...(headers === undefined ? {} : { headers }),
      toolPolicy: {
        roots: ['.'],
        commands: [],
      },
    },
  };
}

function providerOptions(provider, cwd) {
  if (provider === 'gateway') return liveGatewayOptions(cwd);

  return {
    cwd,
    outputFormat: 'stream-json',
  };
}

function eventText(event) {
  if (!event || typeof event !== 'object') return '';
  if (event.type === 'text' || event.type === 'thinking') return event.text || '';
  if (event.type === 'result') {
    return [event.result, event.text, event.message].filter(Boolean).join('\n');
  }
  return '';
}

function summarizeResult(envelope) {
  if (!envelope || typeof envelope !== 'object') return 'no envelope';
  if (!envelope.ok) {
    const error = envelope.error || {};
    return `${error.code || 'error'}: ${error.message || 'unknown failure'}`;
  }

  const result = envelope.result || {};
  const events = Array.isArray(result.events) ? result.events : [];
  const text = events.map(eventText).join('\n').trim();
  const last = events.at(-1);
  return JSON.stringify(
    {
      provider: envelope.provider,
      exitCode: result.exitCode,
      signal: result.signal,
      timedOut: result.timedOut,
      eventCount: events.length,
      lastEventType: last && typeof last === 'object' ? last.type : null,
      text: text.slice(0, 500),
    },
    null,
    2
  );
}

function assertSmokePassed(provider, response) {
  if (response.exitCode !== 0) {
    throw new Error(
      `${provider} contract exited ${response.exitCode}: ${summarizeResult(response.envelope)}`
    );
  }
  if (!response.envelope.ok) {
    throw new Error(`${provider} returned a contract error: ${summarizeResult(response.envelope)}`);
  }

  const result = response.envelope.result || {};
  const events = Array.isArray(result.events) ? result.events : [];
  const text = events.map(eventText).join('\n');
  const finalResult = events.findLast((event) => event && event.type === 'result');

  if (result.timedOut) {
    throw new Error(`${provider} timed out after ${result.timeoutMs || TIMEOUT_MS}ms.`);
  }
  if (finalResult && finalResult.success === false) {
    throw new Error(
      `${provider} produced a failed result event: ${summarizeResult(response.envelope)}`
    );
  }
  if (!text.includes(SENTINEL)) {
    throw new Error(
      `${provider} did not return sentinel ${SENTINEL}.\nResult:\n${summarizeResult(response.envelope)}`
    );
  }
}

async function smokeProvider(provider) {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), `zeroshot-live-${provider}-`));
  try {
    const prompt = process.env.ZEROSHOT_LIVE_PROMPT || DEFAULT_PROMPT;
    const response = await helper.runProviderExecutable(
      JSON.stringify({
        schemaVersion: 1,
        command: 'invoke',
        provider,
        context: prompt,
        options: providerOptions(provider, tempDir),
        timeoutMs: TIMEOUT_MS,
      })
    );

    assertSmokePassed(provider, response);
    console.log(`✓ ${provider} live smoke returned ${SENTINEL}`);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

async function main() {
  const providers = parseProviders();
  console.log(`Running live provider smoke for: ${providers.join(', ')}`);
  console.log(`Sentinel: ${SENTINEL}`);

  for (const provider of providers) {
    await smokeProvider(provider);
  }
}

main().catch((error) => {
  fail(error.stack || error.message || String(error));
});
