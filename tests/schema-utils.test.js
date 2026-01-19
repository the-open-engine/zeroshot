/**
 * Tests for schema-utils.js - enum normalization
 *
 * PROBLEM: LLMs return "simple" instead of "SIMPLE", causing schema validation failures.
 * SOLUTION: Normalize enum values before validation.
 */

const { expect } = require('chai');
const { normalizeEnumValues } = require('../src/agent/schema-utils');

const conductorSchema = {
  type: 'object',
  properties: {
    complexity: {
      type: 'string',
      enum: ['TRIVIAL', 'SIMPLE', 'STANDARD', 'CRITICAL', 'UNCERTAIN'],
    },
    taskType: {
      type: 'string',
      enum: ['INQUIRY', 'TASK', 'DEBUG'],
    },
    reasoning: {
      type: 'string',
    },
  },
  required: ['complexity', 'taskType', 'reasoning'],
};

function registerCaseNormalizationTests() {
  describe('case normalization', () => {
    it('normalizes lowercase to uppercase', () => {
      const result = { complexity: 'simple', taskType: 'task', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('SIMPLE');
      expect(result.taskType).to.equal('TASK');
    });

    it('normalizes mixed case to correct case', () => {
      const result = { complexity: 'Simple', taskType: 'Task', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('SIMPLE');
      expect(result.taskType).to.equal('TASK');
    });

    it('preserves already correct values', () => {
      const result = { complexity: 'STANDARD', taskType: 'DEBUG', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('STANDARD');
      expect(result.taskType).to.equal('DEBUG');
    });
  });
}

function registerWhitespaceHandlingTests() {
  describe('whitespace handling', () => {
    it('trims leading/trailing whitespace', () => {
      const result = { complexity: ' SIMPLE ', taskType: '  TASK  ', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('SIMPLE');
      expect(result.taskType).to.equal('TASK');
    });
  });
}

function registerCommonVariationTests() {
  describe('common variations', () => {
    it('maps BUG to DEBUG', () => {
      const result = { complexity: 'SIMPLE', taskType: 'BUG', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.taskType).to.equal('DEBUG');
    });

    it('maps FIX to DEBUG', () => {
      const result = { complexity: 'SIMPLE', taskType: 'fix', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.taskType).to.equal('DEBUG');
    });

    it('maps BUGFIX to DEBUG', () => {
      const result = { complexity: 'SIMPLE', taskType: 'bugfix', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.taskType).to.equal('DEBUG');
    });

    it('maps IMPLEMENT to TASK', () => {
      const result = { complexity: 'SIMPLE', taskType: 'implement', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.taskType).to.equal('TASK');
    });

    it('maps CREATE to TASK', () => {
      const result = { complexity: 'SIMPLE', taskType: 'create', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.taskType).to.equal('TASK');
    });

    it('maps QUESTION to INQUIRY', () => {
      const result = { complexity: 'SIMPLE', taskType: 'question', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.taskType).to.equal('INQUIRY');
    });

    it('maps EASY to TRIVIAL', () => {
      const result = { complexity: 'easy', taskType: 'TASK', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('TRIVIAL');
    });

    it('maps MODERATE to STANDARD', () => {
      const result = { complexity: 'moderate', taskType: 'TASK', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('STANDARD');
    });

    it('maps COMPLEX to CRITICAL', () => {
      const result = { complexity: 'complex', taskType: 'TASK', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('CRITICAL');
    });
  });
}

function registerPipeSeparatedFormatTests() {
  describe('pipe-separated format detection', () => {
    it('extracts first valid value from copied enum list', () => {
      const result = {
        complexity: 'TRIVIAL|SIMPLE|STANDARD|CRITICAL|UNCERTAIN',
        taskType: 'INQUIRY|TASK|DEBUG',
        reasoning: 'test',
      };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('TRIVIAL');
      expect(result.taskType).to.equal('INQUIRY');
    });

    it('handles partial pipe-separated values', () => {
      const result = {
        complexity: 'SIMPLE|STANDARD',
        taskType: 'TASK',
        reasoning: 'test',
      };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal('SIMPLE');
      expect(result.taskType).to.equal('TASK');
    });

    it('ignores single pipe that is not enum list', () => {
      const result = {
        complexity: 'SIMPLE',
        taskType: 'TASK',
        reasoning: 'test | with pipe',
      };
      normalizeEnumValues(result, conductorSchema);
      expect(result.reasoning).to.equal('test | with pipe');
    });
  });
}

function registerEdgeCaseTests() {
  describe('edge cases', () => {
    it('handles null result', () => {
      const result = normalizeEnumValues(null, conductorSchema);
      expect(result).to.be.null;
    });

    it('handles undefined result', () => {
      const result = normalizeEnumValues(undefined, conductorSchema);
      expect(result).to.be.undefined;
    });

    it('handles null schema', () => {
      const result = { complexity: 'simple', taskType: 'task' };
      normalizeEnumValues(result, null);
      // Should not throw, values unchanged
      expect(result.complexity).to.equal('simple');
    });

    it('handles schema without properties', () => {
      const result = { complexity: 'simple' };
      normalizeEnumValues(result, { type: 'object' });
      expect(result.complexity).to.equal('simple');
    });

    it('ignores non-string values', () => {
      const result = { complexity: 123, taskType: null, reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.complexity).to.equal(123);
      expect(result.taskType).to.be.null;
    });

    it('ignores unknown variations', () => {
      const result = { complexity: 'UNKNOWN_VALUE', taskType: 'TASK', reasoning: 'test' };
      normalizeEnumValues(result, conductorSchema);
      // Should not change - unknown value stays as-is
      expect(result.complexity).to.equal('UNKNOWN_VALUE');
    });

    it('does not touch non-enum string fields', () => {
      const result = { complexity: 'SIMPLE', taskType: 'TASK', reasoning: 'Some reasoning text' };
      normalizeEnumValues(result, conductorSchema);
      expect(result.reasoning).to.equal('Some reasoning text');
    });
  });
}

function registerNestedObjectTests() {
  describe('nested objects', () => {
    const nestedSchema = {
      type: 'object',
      properties: {
        outer: {
          type: 'object',
          properties: {
            status: {
              type: 'string',
              enum: ['PENDING', 'RUNNING', 'COMPLETE'],
            },
          },
        },
      },
    };

    it('normalizes nested object enums', () => {
      const result = { outer: { status: 'running' } };
      normalizeEnumValues(result, nestedSchema);
      expect(result.outer.status).to.equal('RUNNING');
    });
  });
}

function registerArrayObjectTests() {
  describe('arrays of objects', () => {
    const arraySchema = {
      type: 'object',
      properties: {
        items: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              status: {
                type: 'string',
                enum: ['PENDING', 'RUNNING', 'COMPLETE'],
              },
            },
          },
        },
      },
    };

    it('normalizes enums in array items', () => {
      const result = {
        items: [{ status: 'pending' }, { status: 'running' }, { status: 'complete' }],
      };
      normalizeEnumValues(result, arraySchema);
      expect(result.items[0].status).to.equal('PENDING');
      expect(result.items[1].status).to.equal('RUNNING');
      expect(result.items[2].status).to.equal('COMPLETE');
    });
  });
}

describe('normalizeEnumValues', () => {
  registerCaseNormalizationTests();
  registerWhitespaceHandlingTests();
  registerCommonVariationTests();
  registerPipeSeparatedFormatTests();
  registerEdgeCaseTests();
  registerNestedObjectTests();
  registerArrayObjectTests();
});
