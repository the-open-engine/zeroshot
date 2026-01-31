const { RPC_ERROR_CODES, RPC_ERROR_MESSAGES } = require("./constants");

const buildError = (code: number, message: string, detail?: string) => {
  const error: any = { code, message };
  if (detail) {
    error.data = { detail };
  }
  return error;
};

const createDispatcher = (options: any = {}) => {
  const serverInfo = options.serverInfo || { name: "zeroshot", version: "0.0.0" };
  const protocolVersion =
    typeof options.protocolVersion === "number" ? options.protocolVersion : 1;
  const methods = ["initialize", "ping"];
  const notifications = [];

  const handlers = {
    initialize: async () => ({
      protocolVersion,
      server: serverInfo,
      capabilities: {
        methods,
        notifications,
      },
    }),
    ping: async () => ({ ok: true }),
  };

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
      const detail =
        error instanceof Error ? error.message : "Unhandled dispatcher error";
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
