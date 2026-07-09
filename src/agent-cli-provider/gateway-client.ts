import { getArray, getRecord, getString, isRecord, unknownToMessage } from './json';

export interface GatewayChatToolDefinition {
  readonly type: 'function';
  readonly function: {
    readonly name: string;
    readonly description: string;
    readonly parameters: Record<string, unknown>;
  };
}

export interface GatewayToolCall {
  readonly id: string;
  readonly name: string;
  readonly argumentsText: string;
}

export interface GatewayChatMessage {
  readonly role: 'system' | 'user' | 'assistant' | 'tool';
  readonly content: string;
  readonly toolCalls?: readonly GatewayToolCall[];
  readonly toolCallId?: string;
}

export interface GatewayChatResponse {
  readonly text: string;
  readonly toolCalls: readonly GatewayToolCall[];
}

class GatewayHttpError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'GatewayHttpError';
    this.status = status;
  }
}

export async function createGatewayChatCompletion(input: {
  readonly baseUrl: string;
  readonly apiKey: string;
  readonly headers: Readonly<Record<string, string>>;
  readonly model: string;
  readonly messages: readonly GatewayChatMessage[];
  readonly tools: readonly GatewayChatToolDefinition[];
}): Promise<GatewayChatResponse> {
  const response = await fetch(`${input.baseUrl}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${input.apiKey}`,
      ...input.headers,
    },
    body: JSON.stringify({
      model: input.model,
      messages: input.messages.map((message) => serializeMessage(message)),
      tools: input.tools,
      tool_choice: 'auto',
      temperature: 0,
    }),
  });

  const bodyText = await response.text();
  const parsed = tryParseJson(bodyText);
  if (!response.ok) {
    throw httpError(response.status, parsed ?? bodyText);
  }
  if (!isRecord(parsed)) {
    throw new Error('Gateway returned a non-JSON response.');
  }

  const choice = getArray(parsed, 'choices')[0];
  if (!isRecord(choice)) {
    throw new Error('Gateway response did not include choices[0].');
  }
  const message = getRecord(choice, 'message');
  if (message === null) {
    throw new Error('Gateway response did not include choices[0].message.');
  }

  return {
    text: getGatewayMessageText(message),
    toolCalls: getGatewayToolCalls(message),
  };
}

function serializeMessage(message: GatewayChatMessage): Record<string, unknown> {
  if (message.role === 'assistant' && message.toolCalls && message.toolCalls.length > 0) {
    return {
      role: 'assistant',
      content: message.content,
      tool_calls: message.toolCalls.map((toolCall) => ({
        id: toolCall.id,
        type: 'function',
        function: {
          name: toolCall.name,
          arguments: toolCall.argumentsText,
        },
      })),
    };
  }
  if (message.role === 'tool') {
    return {
      role: 'tool',
      tool_call_id: message.toolCallId,
      content: message.content,
    };
  }
  return {
    role: message.role,
    content: message.content,
  };
}

function getGatewayMessageText(message: Record<string, unknown>): string {
  const content = message.content;
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';

  let text = '';
  for (const item of content) {
    if (!isRecord(item)) continue;
    const type = getString(item, 'type');
    if (type === 'text') {
      text += getString(item, 'text') ?? '';
    }
  }
  return text;
}

function getGatewayToolCalls(message: Record<string, unknown>): readonly GatewayToolCall[] {
  const result: GatewayToolCall[] = [];
  for (const item of getArray(message, 'tool_calls')) {
    if (!isRecord(item)) continue;
    const id = getString(item, 'id');
    const fn = getRecord(item, 'function');
    const name = fn === null ? null : getString(fn, 'name');
    const argumentsText = fn === null ? null : getString(fn, 'arguments');
    if (!id || !name || argumentsText === null) continue;
    result.push({ id, name, argumentsText });
  }
  return result;
}

function httpError(status: number, body: unknown): GatewayHttpError {
  return new GatewayHttpError(status, buildGatewayErrorMessage(status, body));
}

function buildGatewayErrorMessage(status: number, body: unknown): string {
  if (isRecord(body)) {
    const nested = getRecord(body, 'error');
    if (nested) {
      const message = getString(nested, 'message');
      if (message) return `Gateway request failed with status ${status}: ${message}`;
      return `Gateway request failed with status ${status}: ${unknownToMessage(nested)}`;
    }
    const message = getString(body, 'message');
    if (message) return `Gateway request failed with status ${status}: ${message}`;
  }
  return `Gateway request failed with status ${status}: ${unknownToMessage(body)}`;
}

function tryParseJson(value: string): unknown | null {
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return null;
  }
}
