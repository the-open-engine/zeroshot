/**
 * Preflight Runtime Validation Tests
 *
 * Tests the integration of runtime simulation into preflight checks.
 * Verifies that cluster configs are validated BEFORE execution starts,
 * catching bugs like:
 * - Consensus gates firing early on duplicate messages
 * - Missing completion handlers
 * - Incorrect trigger logic
 * - Dead topics
 */

const { expect } = require('chai');
const { runPreflight, validateClusterConfig } = require('../src/preflight');

describe('Preflight Runtime Validation', function () {
  // Allow slower tests for simulation (runtime simulation + preflight checks can take 10-15s)
  this.timeout(20000);

  describe('validateClusterConfig()', () => {
    it('should pass validation for valid quick-validation config', async () => {
      const config = {
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            modelLevel: 'level2',
            triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'IMPLEMENTATION_READY' },
              },
            },
          },
          {
            id: 'validator-requirements',
            role: 'validator',
            modelLevel: 'level1',
            triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'QUICK_VALIDATION_RESULT' },
              },
            },
          },
          {
            id: 'validator-code',
            role: 'validator',
            modelLevel: 'level1',
            triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'QUICK_VALIDATION_RESULT' },
              },
            },
          },
          {
            id: 'consensus-coordinator',
            role: 'coordinator',
            modelLevel: 'level1',
            triggers: [
              {
                topic: 'QUICK_VALIDATION_RESULT',
                logic: {
                  engine: 'javascript',
                  script: `
                    const validators = cluster.getAgentsByRole('validator')
                      .filter(v => {
                        const hookTopic = v.config?.hooks?.onComplete?.config?.topic || v.hooks?.onComplete?.config?.topic;
                        return hookTopic === 'QUICK_VALIDATION_RESULT';
                      });
                    const results = ledger.query({ topic: 'QUICK_VALIDATION_RESULT' });
                    const senders = new Set(results.map(m => m.sender));
                    const allResponded = validators.every(v => senders.has(v.id));
                    if (!allResponded) return false;

                    const uniqueSenders = Array.from(senders);
                    const validatorResults = uniqueSenders.map(senderId => {
                      return results.find(m => m.sender === senderId);
                    });

                    return validatorResults.length === validators.length;
                  `,
                },
                action: 'execute_task',
              },
            ],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'QUICK_VALIDATION_PASSED' },
                transform: {
                  engine: 'javascript',
                  script: `
                    const validators = cluster.getAgentsByRole('validator')
                      .filter(v => {
                        const hookTopic = v.config?.hooks?.onComplete?.config?.topic || v.hooks?.onComplete?.config?.topic;
                        return hookTopic === 'QUICK_VALIDATION_RESULT';
                      });
                    const results = ledger.query({ topic: 'QUICK_VALIDATION_RESULT' });
                    const senders = new Set(results.map(m => m.sender));
                    const uniqueSenders = Array.from(senders);
                    const validatorResults = uniqueSenders.map(senderId => {
                      return results.find(m => m.sender === senderId);
                    });

                    const allApproved = validatorResults.every(msg => msg.content?.data?.approved === true);

                    if (allApproved) {
                      return {
                        topic: 'QUICK_VALIDATION_PASSED',
                        content: {
                          text: 'All validators approved',
                          data: { allApproved: true },
                        },
                      };
                    } else {
                      const errors = validatorResults.flatMap(msg => msg.content?.data?.errors || []);
                      return {
                        topic: 'VALIDATION_RESULT',
                        content: {
                          text: 'Validation failed',
                          data: { approved: false, errors },
                        },
                      };
                    }
                  `,
                },
              },
            },
          },
          {
            id: 'completion-detector',
            role: 'orchestrator',
            modelLevel: 'level1',
            triggers: [
              {
                topic: 'VALIDATION_RESULT',
                action: 'stop_cluster',
              },
              {
                topic: 'QUICK_VALIDATION_PASSED',
                action: 'stop_cluster',
              },
            ],
          },
        ],
      };

      const result = await validateClusterConfig(config, 'quick-validation-test');

      expect(result.errors).to.be.an('array').that.is.empty;
      expect(result.warnings).to.be.an('array');
    });

    it('should detect consensus gate firing early on duplicate sender', async () => {
      const config = {
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            modelLevel: 'level2',
            triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'IMPLEMENTATION_READY' },
              },
            },
          },
          {
            id: 'validator-1',
            role: 'validator',
            modelLevel: 'level1',
            triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'VALIDATION_RESULT' },
              },
            },
          },
          {
            id: 'validator-2',
            role: 'validator',
            modelLevel: 'level1',
            triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'VALIDATION_RESULT' },
              },
            },
          },
          {
            id: 'consensus-coordinator',
            role: 'coordinator',
            modelLevel: 'level1',
            triggers: [
              {
                topic: 'VALIDATION_RESULT',
                logic: {
                  engine: 'javascript',
                  // BUGGY: Fires after count >= 2, doesn't check distinct senders
                  script: `
                    const results = ledger.query({ topic: 'VALIDATION_RESULT' });
                    return results.length >= 2;
                  `,
                },
                action: 'stop_cluster',
              },
            ],
          },
        ],
      };

      const result = await validateClusterConfig(config, 'buggy-consensus');

      // Should detect that consensus gate fires early on duplicate messages
      expect(result.errors).to.be.an('array');
      const consensusError = result.errors.find((err) =>
        err.includes('fires early on duplicate sender')
      );
      expect(consensusError).to.exist;
    });

    it('should skip validation for configs without agents', async () => {
      const config = { name: 'empty-config' };
      const result = await validateClusterConfig(config, 'empty');

      expect(result.errors).to.be.an('array').that.is.empty;
      expect(result.warnings).to.be.an('array').that.is.empty;
    });
  });

  describe('runPreflight() with cluster config', () => {
    it('should validate cluster config when provided', async () => {
      const config = {
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            modelLevel: 'level2',
            triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'DONE' },
              },
            },
          },
          {
            id: 'stopper',
            role: 'orchestrator',
            modelLevel: 'level1',
            triggers: [{ topic: 'DONE', action: 'stop_cluster' }],
          },
        ],
      };

      const result = await runPreflight({
        clusterConfig: config,
        templateId: 'test-cluster',
        quiet: true,
      });

      expect(result.valid).to.be.true;
      expect(result.errors).to.be.an('array').that.is.empty;
    });

    it('should report validation errors in preflight result', async function () {
      this.timeout(30000); // Increase timeout for this specific test
      const config = {
        agents: [
          {
            id: 'worker',
            role: 'implementation',
            modelLevel: 'level2',
            triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'IMPLEMENTATION_READY' },
              },
            },
          },
          {
            id: 'validator-1',
            role: 'validator',
            modelLevel: 'level1',
            triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'VALIDATION_RESULT' },
              },
            },
          },
          {
            id: 'validator-2',
            role: 'validator',
            modelLevel: 'level1',
            triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'VALIDATION_RESULT' },
              },
            },
          },
          {
            id: 'consensus-coordinator',
            role: 'coordinator',
            modelLevel: 'level1',
            triggers: [
              {
                topic: 'VALIDATION_RESULT',
                logic: {
                  engine: 'javascript',
                  // BUGGY: count-based consensus (fires early on duplicates)
                  script: `
                    const results = ledger.query({ topic: 'VALIDATION_RESULT' });
                    return results.length >= 2;
                  `,
                },
                action: 'stop_cluster',
              },
            ],
          },
        ],
      };

      const result = await runPreflight({
        clusterConfig: config,
        templateId: 'buggy-cluster',
        quiet: true,
      });

      expect(result.valid).to.be.false;
      expect(result.errors).to.be.an('array').that.is.not.empty;
      const consensusError = result.errors.find((err) =>
        err.includes('fires early on duplicate sender')
      );
      expect(consensusError).to.exist;
    });
  });
});
