// Native-event peers only: no provider requests or actual tool execution.
const path = require('node:path');
const harness = path.basename(__filename, '.cjs').replace('tool-', '');
const credential = harness === 'pi' ? 8675309123 : 'fixture-key';
const emit = (value) => process.stdout.write(JSON.stringify(value) + '\n');
const response = JSON.stringify({ response: null });
if (process.argv.includes('--version')) {
  console.log(
    {
      codex: 'codex-cli 0.155.0',
      claude: '2.1.310 (Claude Code)',
      pi: '1.4.0',
      copilot: '1.0.70',
    }[harness]
  );
} else if (harness === 'codex' && process.argv.includes('app-server')) {
  require('./harness.cjs');
} else if (harness === 'copilot') {
  let pending = Buffer.alloc(0);
  let session;
  const send = (message) => {
    const body = JSON.stringify({ jsonrpc: '2.0', ...message });
    process.stdout.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
  };
  const event = (type, data) =>
    send({
      method: 'session.event',
      params: {
        sessionId: session,
        event: { type, data },
      },
    });
  process.stdin.on('data', (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    for (;;) {
      const end = pending.indexOf('\r\n\r\n');
      if (end < 0) break;
      const length = Number(/Content-Length: (\d+)/i.exec(pending.subarray(0, end).toString())[1]);
      if (pending.length < end + 4 + length) break;
      const message = JSON.parse(pending.subarray(end + 4, end + 4 + length));
      pending = pending.subarray(end + 4 + length);
      let result = {};
      if (message.method === 'connect') result = { protocolVersion: 3 };
      if (message.method === 'session.create' || message.method === 'session.resume') {
        session = message.params.sessionId;
        result = { sessionId: session };
      }
      if (message.method === 'session.send') {
        event('assistant.message', { content: 'commentary-before' });
        for (const failed of [false, true]) {
          const id = failed ? 'native-fail' : 'native-ok';
          const call = { toolCallId: id, toolName: 'fixture-shell', arguments: input(id) };
          event('tool.execution_start', call);
          event('tool.execution_start', call);
          event('tool.execution_partial_result', {
            toolCallId: id,
            partialOutput: 'ignored-cumulative',
          });
          const complete = {
            toolCallId: id,
            success: !failed,
            result: { content: output(id) },
            ...(failed ? { error: { message: 'native-tool-error' } } : {}),
          };
          event('tool.execution_complete', complete);
          event('tool.execution_complete', complete);
        }
        event('assistant.message', { content: response });
        result = { messageId: 'final' };
      }
      if (message.method === 'session.detach') result = { success: true };
      send({ id: message.id, result });
    }
  });
} else {
  let started = false;
  const run = () => {
    if (started) return;
    started = true;
    if (harness === 'claude') {
      emit({ type: 'system', subtype: 'init', session_id: 'fixture-session' });
      emit({
        type: 'stream_event',
        event: {
          type: 'content_block_delta',
          index: 0,
          delta: { type: 'text_delta', text: 'commentary-before' },
        },
      });
    }
    for (const failed of [false, true]) {
      const id = failed ? 'native-fail' : 'native-ok';
      if (harness === 'claude') {
        emit({
          type: 'stream_event',
          event: {
            type: 'content_block_start',
            index: 1,
            content_block: { type: 'tool_use', id, name: 'fixture-shell', input: {} },
          },
        });
        emit({
          type: 'stream_event',
          event: {
            type: 'content_block_delta',
            index: 1,
            delta: { type: 'input_json_delta', partial_json: JSON.stringify(input(id)) },
          },
        });
        const call = {
          type: 'assistant',
          message: {
            content: [
              { type: 'text', text: 'commentary-before' },
              { type: 'tool_use', id, name: 'fixture-shell', input: input(id) },
            ],
          },
        };
        emit(call);
        emit(call);
        const result = {
          type: 'user',
          message: {
            content: [
              { type: 'tool_result', tool_use_id: id, content: output(id), is_error: failed },
            ],
          },
        };
        emit(result);
        emit(result);
      } else if (harness === 'pi') {
        const call = {
          type: 'tool_execution_start',
          toolCallId: id,
          toolName: 'fixture-shell',
          args: input(id),
        };
        emit(call);
        emit(call);
        emit({
          type: 'tool_execution_update',
          toolCallId: id,
          partialResult: 'ignored-cumulative',
        });
        const result = {
          type: 'tool_execution_end',
          toolCallId: id,
          toolName: 'fixture-shell',
          result: { content: [{ type: 'text', text: output(id) }], details: { credential } },
          isError: failed,
        };
        emit(result);
        emit(result);
      } else {
        const item = {
          id,
          type: 'command_execution',
          command: input(id).command,
          status: 'in_progress',
        };
        emit({ type: 'item.started', item });
        emit({ type: 'item.started', item });
        emit({ type: 'item.updated', item: { ...item, aggregated_output: 'ignored-cumulative' } });
        const complete = {
          type: 'item.completed',
          item: {
            ...item,
            aggregated_output: output(id),
            status: failed ? 'failed' : 'completed',
            exit_code: failed ? 1 : 0,
          },
        };
        emit(complete);
        emit(complete);
        const collab = {
          id: `collab-${id}`,
          type: 'collab_tool_call',
          tool: failed ? 'wait' : 'spawn_agent',
          sender_thread_id: 'fixture-parent',
          receiver_thread_ids: ['fixture-child'],
          prompt: failed ? null : `inspect-critical.rs ${credential}`,
          agents_states: {},
          status: 'in_progress',
        };
        emit({ type: 'item.started', item: collab });
        emit({ type: 'item.started', item: collab });
        emit({ type: 'item.updated', item: { ...collab, prompt: 'ignored-cumulative' } });
        const collabComplete = {
          type: 'item.completed',
          item: {
            ...collab,
            status: failed ? 'failed' : 'completed',
            agents_states: {
              'fixture-child': {
                status: failed ? 'errored' : 'completed',
                message: `collab-result-${id} ${credential}`,
              },
            },
          },
        };
        emit(collabComplete);
        emit(collabComplete);
      }
    }
    if (harness === 'claude') {
      emit({
        type: 'result',
        subtype: 'success',
        session_id: 'fixture-session',
        structured_output: { response: null },
      });
    } else if (harness === 'pi') {
      emit({
        type: 'message_update',
        assistantMessageEvent: { type: 'text_delta', delta: response },
      });
      emit({
        type: 'message_end',
        message: {
          role: 'assistant',
          content: [{ type: 'text', text: response }],
          stopReason: 'stop',
        },
      });
    } else {
      const search = {
        id: 'native-web',
        type: 'web_search',
        query: 'first-query ...',
        action: { type: 'search', queries: ['first-query', `second-query ${credential}`] },
      };
      for (const type of ['item.started', 'item.started', 'item.completed', 'item.completed']) {
        emit({ type, item: search });
      }
      emit({ type: 'thread.started', thread_id: 'fixture-session' });
      emit({ type: 'item.completed', item: { type: 'agent_message', text: response } });
      emit({ type: 'turn.completed' });
    }
    process.stdout.write('', () => process.exit(0));
  };
  process.stdin.resume();
  // Claude keeps streaming stdin open; Pi/Codex consume the prompt through EOF.
  process.stdin.once(harness === 'claude' ? 'data' : 'end', run);
}
function input(id) {
  return {
    command: `command-${id} ${credential} ` + '界'.repeat(9000) + ' input-tail',
    credential,
  };
}
function output(id) {
  return `result-${id} ${credential} ` + 'é'.repeat(9000) + ' result-tail';
}
