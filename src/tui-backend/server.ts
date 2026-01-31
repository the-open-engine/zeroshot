const path = require("path");
const {
  createValidator,
  createDispatcher,
  createFrameParser,
  encodeFrame,
  RPC_ERROR_CODES,
  RPC_ERROR_MESSAGES,
  PROTOCOL_VERSION,
} = require("./protocol");

const isValidId = (value) => typeof value === "string" || typeof value === "number";

const loadPackageInfo = () => {
  try {
    const packagePath = path.resolve(__dirname, "..", "..", "package.json");
    const pkg = require(packagePath);
    return {
      name: typeof pkg.name === "string" ? pkg.name : "zeroshot",
      version: typeof pkg.version === "string" ? pkg.version : "0.0.0",
    };
  } catch (error) {
    return { name: "zeroshot", version: "0.0.0" };
  }
};

const writeFrame = (payload) => {
  const framed = encodeFrame(payload);
  process.stdout.write(framed);
};

const writeError = (id, error) => {
  writeFrame({
    jsonrpc: "2.0",
    id,
    error,
  });
};

const logDiagnostic = (message, error) => {
  const details =
    error instanceof Error ? `${message}: ${error.stack || error.message}` : message;
  process.stderr.write(`${details}\n`);
};

const startServer = () => {
  const validator = createValidator();
  const dispatcher = createDispatcher({
    serverInfo: loadPackageInfo(),
    protocolVersion: PROTOCOL_VERSION,
  });
  const parser = createFrameParser();

  const handleFrame = async (payload) => {
    let message;
    try {
      message = JSON.parse(payload);
    } catch (error) {
      writeError(null, {
        code: RPC_ERROR_CODES.PARSE_ERROR,
        message: RPC_ERROR_MESSAGES[RPC_ERROR_CODES.PARSE_ERROR],
      });
      logDiagnostic("Invalid JSON payload", error);
      return;
    }

    if (!message || typeof message !== "object") {
      writeError(null, {
        code: RPC_ERROR_CODES.INVALID_REQUEST,
        message: RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
      });
      return;
    }

    const hasId = Object.prototype.hasOwnProperty.call(message, "id");
    if (!hasId) {
      const notification = validator.validateNotification(message);
      if (!notification.ok) {
        logDiagnostic("Invalid notification received", notification.error);
      }
      return;
    }

    const requestValidation = validator.validateRequest(message);
    if (!requestValidation.ok) {
      const responseId = isValidId(message.id) ? message.id : null;
      writeError(responseId, requestValidation.error);
      return;
    }

    const dispatchResult = await dispatcher.dispatchRequest(requestValidation.value);
    if (!dispatchResult.ok) {
      writeError(message.id, dispatchResult.error);
      return;
    }

    writeFrame({
      jsonrpc: "2.0",
      id: message.id,
      result: dispatchResult.result,
    });
  };

  const handleChunk = (chunk) => {
    let frames = [];
    try {
      frames = parser.push(chunk);
    } catch (error) {
      parser.reset();
      writeError(null, {
        code: RPC_ERROR_CODES.PARSE_ERROR,
        message: RPC_ERROR_MESSAGES[RPC_ERROR_CODES.PARSE_ERROR],
        data: { detail: error instanceof Error ? error.message : "Parse error" },
      });
      logDiagnostic("Frame parsing failed", error);
      return;
    }

    for (const frame of frames) {
      void handleFrame(frame);
    }
  };

  process.stdin.on("data", handleChunk);
  process.stdin.on("end", () => {
    process.exit(0);
  });
  process.stdin.on("error", (error) => {
    logDiagnostic("Stdin error", error);
    process.exit(1);
  });

  process.on("uncaughtException", (error) => {
    logDiagnostic("Uncaught exception", error);
    process.exit(1);
  });
  process.on("unhandledRejection", (error) => {
    logDiagnostic("Unhandled rejection", error);
    process.exit(1);
  });

  process.stdin.resume();
};

if (require.main === module) {
  startServer();
}

module.exports = {
  startServer,
};
