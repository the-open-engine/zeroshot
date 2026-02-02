const { RPC_ERROR_CODES, RPC_ERROR_MESSAGES } = require('./constants');

const buildError = (code: number, message: string, detail?: string) => {
  const error: any = { code, message };
  if (detail) {
    error.data = { detail };
  }
  return error;
};

const isRpcError = (error: any) =>
  error &&
  typeof error === 'object' &&
  typeof error.code === 'number' &&
  typeof error.message === 'string';

const createDispatcher = (options: any = {}) => {
  const serverInfo = options.serverInfo || { name: 'zeroshot', version: '0.0.0' };
  const protocolVersion = typeof options.protocolVersion === 'number' ? options.protocolVersion : 1;
  const baseHandlers = {
    initialize: async () => ({
      protocolVersion,
      server: serverInfo,
      capabilities: {
        methods: [],
        notifications: [],
      },
    }),
    ping: async () => ({ ok: true }),
  };
  const extraHandlers =
    options.handlers && typeof options.handlers === 'object' ? options.handlers : {};
  const handlers = { ...baseHandlers, ...extraHandlers };
  const methods = Array.from(new Set(Object.keys(handlers)));
  const notifications = Array.isArray(options.notifications) ? options.notifications : [];
  handlers.initialize = async () => ({
    protocolVersion,
    server: serverInfo,
    capabilities: {
      methods,
      notifications,
    },
  });

  const dispatchRequest = async (message) => {
    const handler = handlers[message.method];
    if (!handler) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.METHOD_NOT_FOUND,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.METHOD_NOT_FOUND]
        ),
      };
    }
    try {
      const result = await handler(message.params ?? null, message);
      return { ok: true, result };
    } catch (error) {
      if (isRpcError(error)) {
        const rpcError: any = { code: error.code, message: error.message };
        if (error.data) {
          rpcError.data = error.data;
        }
        return { ok: false, error: rpcError };
      }
      const detail = error instanceof Error ? error.message : 'Unhandled dispatcher error';
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INTERNAL_ERROR,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INTERNAL_ERROR],
          detail
        ),
      };
    }
  };

  return {
    dispatchRequest,
    methods,
    notifications,
  };
};

module.exports = {
  createDispatcher,
};
