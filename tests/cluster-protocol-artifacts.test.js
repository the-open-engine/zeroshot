const assert = require('assert');
const fs = require('fs');
const path = require('path');
const Ajv2020 = require('ajv/dist/2020');

const artifactDirectory = path.join(__dirname, '..', 'protocol', 'openengine-cluster', 'v1');

describe('generated cluster protocol artifacts', () => {
  it('validates every golden response against its generated schema definition', () => {
    const schema = JSON.parse(fs.readFileSync(path.join(artifactDirectory, 'schema.json'), 'utf8'));
    const ajv = new Ajv2020({ allErrors: true, strict: false, validateFormats: false });
    const validators = Object.fromEntries(
      ['JsonRpcSuccess', 'JsonRpcSuccess2', 'JsonRpcErrorResponse'].map((definition) => [
        definition,
        ajv.compile({
          $schema: schema.$schema,
          $ref: `#/$defs/${definition}`,
          $defs: schema.$defs,
        }),
      ])
    );
    const goldenDirectory = path.join(artifactDirectory, 'goldens');

    for (const filename of fs.readdirSync(goldenDirectory).sort()) {
      const frames = fs
        .readFileSync(path.join(goldenDirectory, filename), 'utf8')
        .trim()
        .split('\n')
        .map((frame) => JSON.parse(frame));
      assert.strictEqual(frames.length, 2, `${filename} must contain one request and one response`);
      const [request, response] = frames;
      let definition = 'JsonRpcSuccess2';
      if (response.error) {
        definition = 'JsonRpcErrorResponse';
      } else if (request.method === 'initialize') {
        definition = 'JsonRpcSuccess';
      }
      const validate = validators[definition];

      assert(
        validate(response),
        `${filename} response violates ${definition}: ${ajv.errorsText(validate.errors)}`
      );
    }
  });
});
