const assert = require('assert');
const {
  getValidationRuntimePortKeys,
  getValidationRuntimeTemplateParams,
  resolveValidationRuntimeEnv,
} = require('../../src/validation-runtime');

describe('validation-runtime helpers', function () {
  it('should extract requested port bundle keys from runtime env', function () {
    const keys = getValidationRuntimePortKeys({
      env: {
        LOCAL_BACKEND_PORT: '{{ports.backend}}',
        LOCAL_FRONTEND_PORT: '{{ports.frontend}}',
        COMPOSE_PROJECT_NAME: '{{clusterId}}',
      },
    });

    assert.deepStrictEqual(keys, ['backend', 'frontend']);
  });

  it('should resolve clusterId and allocated ports into runtime env', function () {
    const env = resolveValidationRuntimeEnv({
      envConfig: {
        COMPOSE_PROJECT_NAME: '{{clusterId}}',
        LOCAL_BACKEND_PORT: '{{ports.backend}}',
        STATIC_VALUE: 'development',
      },
      clusterId: 'cluster-test-1',
      allocatedPorts: {
        backend: 32101,
      },
    });

    assert.deepStrictEqual(env, {
      COMPOSE_PROJECT_NAME: 'cluster-test-1',
      LOCAL_BACKEND_PORT: '32101',
      STATIC_VALUE: 'development',
    });
  });

  it('should return heavy-template params for runtime-enabled repos', function () {
    assert.deepStrictEqual(getValidationRuntimeTemplateParams(true), {
      include_runtime_validator: true,
      heavy_validator_count: 3,
    });
  });

  it('should return heavy-template params for repos without validation runtime', function () {
    assert.deepStrictEqual(getValidationRuntimeTemplateParams(false), {
      include_runtime_validator: false,
      heavy_validator_count: 2,
    });
  });
});
