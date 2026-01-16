/**
 * Test suite for type-safety validation
 *
 * Ensures template substitution type bugs are caught
 */

const { expect } = require('chai');
const { validateFile } = require('../cluster-scripts/validate-config-type-safety');
const fs = require('fs');
const path = require('path');
const os = require('os');

let tmpDir;

describe('Type Safety Validation', () => {
  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  defineBooleanComparisonTests();
  defineTemplateSubstitutionTests();
  defineRealTemplateValidationTests();
  defineEdgeCaseTests();
});

function defineBooleanComparisonTests() {
  describe('Boolean comparison detection', () => {
    it('should detect unsafe boolean comparison without string fallback', () => {
      const config = {
        agents: [
          {
            id: 'test-agent',
            triggers: [
              {
                topic: 'TEST',
                logic: {
                  script: 'return message.approved === false;',
                },
              },
            ],
          },
        ],
      };

      const filePath = path.join(tmpDir, 'unsafe.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues).to.have.length.greaterThan(0);
      expect(issues[0].severity).to.equal('ERROR');
      expect(issues[0].pattern).to.include('Boolean comparison');
    });

    it('should accept safe boolean comparison with string fallback', () => {
      const config = {
        agents: [
          {
            id: 'test-agent',
            triggers: [
              {
                topic: 'TEST',
                logic: {
                  script: 'return message.approved === false || message.approved === "false";',
                },
              },
            ],
          },
        ],
      };

      const filePath = path.join(tmpDir, 'safe.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues).to.have.length(0);
    });

    it('should handle reverse order (string first, then boolean)', () => {
      const config = {
        agents: [
          {
            id: 'test-agent',
            triggers: [
              {
                topic: 'TEST',
                logic: {
                  script: 'return message.approved === "true" || message.approved === true;',
                },
              },
            ],
          },
        ],
      };

      const filePath = path.join(tmpDir, 'safe-reverse.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues).to.have.length(0);
    });

    it('should handle helper function pattern', () => {
      const config = {
        agents: [
          {
            id: 'test-agent',
            triggers: [
              {
                topic: 'TEST',
                logic: {
                  script:
                    'const approved = (val) => val === true || val === "true"; return approved(message.data);',
                },
              },
            ],
          },
        ],
      };

      const filePath = path.join(tmpDir, 'helper.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues).to.have.length(0);
    });
  });
}

function defineTemplateSubstitutionTests() {
  describe('Template substitution detection', () => {
    it('should warn about boolean template substitution in hooks', () => {
      const config = {
        agents: [
          {
            id: 'validator',
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: {
                  topic: 'RESULT',
                  content: {
                    data: {
                      approved: '{{result.approved}}',
                    },
                  },
                },
              },
            },
          },
          {
            id: 'worker',
            triggers: [
              {
                topic: 'RESULT',
                logic: {
                  script: 'return message.data.approved === true;',
                },
              },
            ],
          },
        ],
      };

      const filePath = path.join(tmpDir, 'template-bug.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues.length).to.be.greaterThan(0);
      const hasTemplateWarning = issues.some((i) => i.hook === 'onComplete');
      const hasTriggerError = issues.some((i) => i.trigger === 'RESULT');
      expect(hasTemplateWarning || hasTriggerError).to.be.true;
    });
  });
}

function defineRealTemplateValidationTests() {
  describe('Real template validation', () => {
    it('should validate full-workflow.json has no issues', () => {
      const templatePath = path.join(
        __dirname,
        '../cluster-templates/base-templates/full-workflow.json'
      );
      if (fs.existsSync(templatePath)) {
        const issues = validateFile(templatePath);
        expect(issues).to.have.length(
          0,
          `full-workflow.json has type-safety issues: ${JSON.stringify(issues, null, 2)}`
        );
      }
    });

    it('should validate debug-workflow.json has no issues', () => {
      const templatePath = path.join(
        __dirname,
        '../cluster-templates/base-templates/debug-workflow.json'
      );
      if (fs.existsSync(templatePath)) {
        const issues = validateFile(templatePath);
        expect(issues).to.have.length(
          0,
          `debug-workflow.json has type-safety issues: ${JSON.stringify(issues, null, 2)}`
        );
      }
    });

    it('should validate worker-validator.json has no issues', () => {
      const templatePath = path.join(
        __dirname,
        '../cluster-templates/base-templates/worker-validator.json'
      );
      if (fs.existsSync(templatePath)) {
        const issues = validateFile(templatePath);
        expect(issues).to.have.length(
          0,
          `worker-validator.json has type-safety issues: ${JSON.stringify(issues, null, 2)}`
        );
      }
    });

    it('should validate agent-library.json has no issues', () => {
      const libraryPath = path.join(__dirname, '../agent-library.json');
      if (fs.existsSync(libraryPath)) {
        const issues = validateFile(libraryPath);
        expect(issues).to.have.length(
          0,
          `agent-library.json has type-safety issues: ${JSON.stringify(issues, null, 2)}`
        );
      }
    });
  });
}

function defineEdgeCaseTests() {
  describe('Edge cases', () => {
    it('should handle malformed JSON gracefully', () => {
      const filePath = path.join(tmpDir, 'malformed.json');
      fs.writeFileSync(filePath, '{invalid json}');

      const issues = validateFile(filePath);

      expect(issues).to.have.length(1);
      expect(issues[0].severity).to.equal('ERROR');
      expect(issues[0].pattern).to.equal('Parse error');
    });

    it('should handle config without agents', () => {
      const config = { name: 'empty' };
      const filePath = path.join(tmpDir, 'empty.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues).to.have.length(0);
    });

    it('should handle agents without triggers', () => {
      const config = {
        agents: [{ id: 'no-triggers' }],
      };
      const filePath = path.join(tmpDir, 'no-triggers.json');
      fs.writeFileSync(filePath, JSON.stringify(config, null, 2));

      const issues = validateFile(filePath);

      expect(issues).to.have.length(0);
    });
  });
}
