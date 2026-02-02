const Ajv = require('ajv');
const { PROTOCOL_VERSION, RPC_ERROR_CODES, RPC_ERROR_MESSAGES } = require('./constants');
const {
  errorSchema,
  jsonRpcRequestBase,
  jsonRpcNotificationBase,
  buildErrorResponseSchema,
  REQUEST_SCHEMAS,
  RESPONSE_SCHEMAS,
  NOTIFICATION_SCHEMAS,
} = require('./schemas');

const buildError = (code, message, errors = []) => {
  const data = Object.create(null);
  if (errors && errors.length) {
    const detail = errors
      .map((err) => {
        const path = err.instancePath || err.schemaPath || '';
        return path ? `${path} ${err.message}` : err.message;
      })
      .join('; ');
    if (detail) {
      data.detail = detail;
    }
    const fields = Object.create(null);
    for (const err of errors) {
      const key = err.instancePath || err.schemaPath || '';
      if (key && !fields[key]) {
        fields[key] = err.message || 'invalid';
      }
    }
    if (Object.keys(fields).length) {
      data.fields = fields;
    }
  }
  const error = Object.create(null);
  error.code = code;
  error.message = message;
  if (Object.keys(data).length) {
    error.data = data;
  }
  return error;
};

const compileSchemaMap = (ajv, schemas) => {
  const validators = new Map();
  for (const [key, schema] of Object.entries(schemas)) {
    validators.set(key, /** @type {any} */ ajv.compile(schema));
  }
  return validators;
};

const createValidator = () => {
  const ajv = new Ajv({
    allErrors: true,
    strict: false,
    coerceTypes: false,
    removeAdditional: false,
  });

  const validateRequestBase = /** @type {any} */ ajv.compile(jsonRpcRequestBase);
  const validateNotificationBase = /** @type {any} */ ajv.compile(jsonRpcNotificationBase);
  const validateErrorObject = /** @type {any} */ ajv.compile(errorSchema);
  const validateErrorResponse = /** @type {any} */ ajv.compile(buildErrorResponseSchema());

  const requestValidators = compileSchemaMap(ajv, REQUEST_SCHEMAS);
  const responseValidators = compileSchemaMap(ajv, RESPONSE_SCHEMAS);
  const notificationValidators = compileSchemaMap(ajv, NOTIFICATION_SCHEMAS);

  const validateRequest = (message) => {
    if (!validateRequestBase(message)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_REQUEST,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
          validateRequestBase.errors || []
        ),
      };
    }

    const validator = requestValidators.get(message.method);
    if (!validator) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.METHOD_NOT_FOUND,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.METHOD_NOT_FOUND]
        ),
      };
    }

    if (!validator(message)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_PARAMS,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
          validator.errors || []
        ),
      };
    }

    if (
      message.method === 'initialize' &&
      message.params &&
      message.params.protocolVersion !== PROTOCOL_VERSION
    ) {
      const error = buildError(
        RPC_ERROR_CODES.PROTOCOL_VERSION_MISMATCH,
        RPC_ERROR_MESSAGES[RPC_ERROR_CODES.PROTOCOL_VERSION_MISMATCH]
      );
      error.data = {
        ...(error.data || {}),
        supportedVersions: [PROTOCOL_VERSION],
      };
      return {
        ok: false,
        error,
      };
    }

    return { ok: true, value: message };
  };

  const validateNotification = (message) => {
    if (!validateNotificationBase(message)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_REQUEST,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
          validateNotificationBase.errors || []
        ),
      };
    }

    const validator = notificationValidators.get(message.method);
    if (!validator) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.METHOD_NOT_FOUND,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.METHOD_NOT_FOUND]
        ),
      };
    }

    if (!validator(message)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_PARAMS,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
          validator.errors || []
        ),
      };
    }

    return { ok: true, value: message };
  };

  const isValidId = (id) => typeof id === 'string' || typeof id === 'number';
  const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

  const validateResponse = (message, method) => {
    if (!isObject(message) || message.jsonrpc !== '2.0' || !isValidId(message.id)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_REQUEST,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
          []
        ),
      };
    }

    const hasError = Object.prototype.hasOwnProperty.call(message, 'error');
    const hasResult = Object.prototype.hasOwnProperty.call(message, 'result');
    if ((hasError && hasResult) || (!hasError && !hasResult)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_REQUEST,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST]
        ),
      };
    }

    if (hasError) {
      if (!validateErrorResponse(message)) {
        return {
          ok: false,
          error: buildError(
            RPC_ERROR_CODES.INVALID_REQUEST,
            RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
            validateErrorResponse.errors || []
          ),
        };
      }
      if (!validateErrorObject(message.error)) {
        return {
          ok: false,
          error: buildError(
            RPC_ERROR_CODES.INVALID_REQUEST,
            RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_REQUEST],
            validateErrorObject.errors || []
          ),
        };
      }
      return { ok: true, value: message };
    }

    const validator = responseValidators.get(method);
    if (!validator) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.METHOD_NOT_FOUND,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.METHOD_NOT_FOUND]
        ),
      };
    }

    if (!validator(message)) {
      return {
        ok: false,
        error: buildError(
          RPC_ERROR_CODES.INVALID_PARAMS,
          RPC_ERROR_MESSAGES[RPC_ERROR_CODES.INVALID_PARAMS],
          validator.errors || []
        ),
      };
    }

    return { ok: true, value: message };
  };

  return {
    validateRequest,
    validateNotification,
    validateResponse,
  };
};

module.exports = {
  createValidator,
  RPC_ERROR_CODES,
  PROTOCOL_VERSION,
};
