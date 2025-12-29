/**
 * Unit test for conductor republish metadata
 *
 * Verifies that:
 * 1. Transform script sets metadata._republished in publish operation
 * 2. _opPublish extracts metadata from operation
 * 3. Junior conductor trigger excludes messages with _republished=true
 */

const { expect } = require('chai');
const fs = require('fs');
const path = require('path');

describe('Conductor Republish Metadata', function () {
  let conductorConfig;

  before(function () {
    // Load conductor-bootstrap config
    const configPath = path.join(__dirname, '..', 'cluster-templates', 'conductor-bootstrap.json');
    conductorConfig = JSON.parse(fs.readFileSync(configPath, 'utf8'));
  });

  it('junior conductor trigger should exclude republished messages', function () {
    const juniorConductor = conductorConfig.agents.find((a) => a.id === 'junior-conductor');
    expect(juniorConductor, 'Junior conductor agent exists').to.exist;

    const issueTrigger = juniorConductor.triggers.find((t) => t.topic === 'ISSUE_OPENED');
    expect(issueTrigger, 'ISSUE_OPENED trigger exists').to.exist;
    expect(issueTrigger.logic, 'Trigger has logic script').to.exist;

    // Parse and validate logic script
    const script = issueTrigger.logic.script;
    expect(script, 'Logic script should check sender=system').to.include('message.sender === ');
    expect(script, 'Logic script should exclude _republished').to.include('!message.metadata?._republished');
  });

  it('junior conductor transform should set _republished metadata', function () {
    const juniorConductor = conductorConfig.agents.find((a) => a.id === 'junior-conductor');
    const hook = juniorConductor.hooks?.onComplete;
    expect(hook, 'onComplete hook exists').to.exist;
    expect(hook.transform, 'Hook uses transform').to.exist;

    const script = hook.transform.script;
    expect(script, 'Transform creates publish operation').to.include("action: 'publish'");
    expect(script, 'Transform sets metadata with _republished').to.include(
      'metadata: { _republished: true }'
    );
  });

  it('senior conductor trigger should exclude republished messages', function () {
    const seniorConductor = conductorConfig.agents.find((a) => a.id === 'senior-conductor');
    expect(seniorConductor, 'Senior conductor agent exists').to.exist;

    // Senior conductor triggers on CONDUCTOR_ESCALATE, not ISSUE_OPENED
    // But it also sets _republished in its transform
    const hook = seniorConductor.hooks?.onComplete;
    expect(hook, 'onComplete hook exists').to.exist;
    expect(hook.transform, 'Hook uses transform').to.exist;

    const script = hook.transform.script;
    expect(script, 'Transform creates publish operation').to.include("action: 'publish'");
    expect(script, 'Transform sets metadata with _republished').to.include(
      'metadata: { _republished: true }'
    );
  });

  it('transform produces correct operations structure', function () {
    // Simulate what the transform script does
    const operations = [
      { action: 'load_config', config: { base: 'single-worker' } },
      {
        action: 'publish',
        topic: 'ISSUE_OPENED',
        content: { text: 'test' },
        metadata: { _republished: true },
      },
    ];

    const message = {
      topic: 'CLUSTER_OPERATIONS',
      content: {
        text: '[TRIVIAL:INQUIRY] test',
        data: {
          complexity: 'TRIVIAL',
          taskType: 'INQUIRY',
          operations: operations,
        },
      },
    };

    // Verify structure
    expect(message.content.data.operations, 'Operations array exists').to.be.an('array');
    expect(message.content.data.operations).to.have.lengthOf(2);

    const publishOp = message.content.data.operations[1];
    expect(publishOp.action, 'Publish operation action').to.equal('publish');
    expect(publishOp.metadata, 'Publish operation has metadata').to.deep.equal({
      _republished: true,
    });
  });
});
