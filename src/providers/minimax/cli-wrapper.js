#!/usr/bin/env node
/**
 * MiniMax CLI Wrapper
 *
 * Lightweight Node.js script that wraps the MiniMax OpenAI-compatible API,
 * enabling MiniMax as a provider in zeroshot's CLI-based execution pipeline.
 *
 * Usage: node cli-wrapper.js [--model MODEL] [--json] <prompt>
 *
 * Outputs JSON events to stdout matching zeroshot's expected format:
 *   {"type":"text","text":"..."}
 *   {"type":"result","success":true,"inputTokens":N,"outputTokens":N}
 *
 * Requires: MINIMAX_API_KEY environment variable
 */

const MINIMAX_BASE_URL = 'https://api.minimax.io/v1';

function parseArgs(argv) {
  const args = argv.slice(2);
  let model = 'MiniMax-M2.7';
  let jsonMode = false;
  const promptParts = [];

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--model' && i + 1 < args.length) {
      model = args[++i];
    } else if (args[i] === '--json') {
      jsonMode = true;
    } else {
      promptParts.push(args[i]);
    }
  }

  return { model, jsonMode, prompt: promptParts.join(' ') };
}

function emit(event) {
  process.stdout.write(JSON.stringify(event) + '\n');
}

function clampTemperature(temp) {
  if (temp === undefined || temp === null) return 0.01;
  return Math.max(0, Math.min(1, temp));
}

function stripThinkTags(text) {
  return text.replace(/<think>[\s\S]*?<\/think>/g, '').trim();
}

async function run() {
  const apiKey = process.env.MINIMAX_API_KEY;
  if (!apiKey) {
    emit({ type: 'error', error: 'MINIMAX_API_KEY environment variable is required' });
    process.exit(1);
  }

  const { model, prompt } = parseArgs(process.argv);

  if (!prompt) {
    emit({ type: 'error', error: 'No prompt provided' });
    process.exit(1);
  }

  const body = {
    model,
    messages: [{ role: 'user', content: prompt }],
    temperature: clampTemperature(0.01),
    stream: false,
  };

  try {
    const response = await fetch(`${MINIMAX_BASE_URL}/chat/completions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorText = await response.text();
      emit({
        type: 'error',
        error: `MiniMax API error (${response.status}): ${errorText}`,
      });
      process.exit(1);
    }

    const data = await response.json();
    const choice = data.choices?.[0];

    if (!choice) {
      emit({ type: 'error', error: 'No response from MiniMax API' });
      process.exit(1);
    }

    const rawText = choice.message?.content || '';
    const text = stripThinkTags(rawText);

    if (text) {
      emit({ type: 'text', text });
    }

    emit({
      type: 'result',
      success: true,
      inputTokens: data.usage?.prompt_tokens || 0,
      outputTokens: data.usage?.completion_tokens || 0,
    });
  } catch (err) {
    emit({
      type: 'error',
      error: `MiniMax API request failed: ${err.message}`,
    });
    process.exit(1);
  }
}

run();
