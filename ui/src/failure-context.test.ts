import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { RunFailureDetails } from './RunFailureDetails';
import { validFailureContext } from './failure-context';
import { validateFailureContext } from './history-contract';

test('native optional failure fields use their defaults through validation and rendering', () => {
  const context = { primary: { message: 'Runtime failed' } };
  assert.ok(validFailureContext(context));
  validateFailureContext(context, 'v2:0');
  const markup = renderToStaticMarkup(
    createElement(RunFailureDetails, {
      context,
      openLog: () => assert.fail('no cursor was supplied'),
      hasLog: () => false,
    })
  );
  assert.match(markup, /Runtime failed/);
  assert.doesNotMatch(markup, /Recorded failures|Additional failure details/);
  assert.deepEqual(
    context,
    { primary: { message: 'Runtime failed' } },
    'raw evidence is unchanged'
  );
});

test('verifier evidence renders literal graph names, nested paths, loop rounds and retained cursors', () => {
  const context = {
    verifiers: [
      {
        message: '{"signals":{"readiness":"revise"},"diagnostic":"missing test"}',
        execution: { node: 'assessor', execution: 'stable-id', mapIndices: [1, 2] },
        cursor: 'v2:7',
      },
    ],
    exhaustedLoops: [{ node: 'refinement', completedIterations: 3, mapIndices: [1] }],
  };
  validateFailureContext(context, 'v2:9');
  const markup = renderToStaticMarkup(
    createElement(RunFailureDetails, { context, openLog: () => {}, hasLog: () => true })
  );
  assert.match(markup, /Recorded verifier results/);
  assert.match(markup, /assessor\[1\]\[2\]/);
  assert.match(markup, /refinement\[1\]/);
  assert.match(markup, /3.*completed iterations/);
  assert.match(markup, /View log/);
  assert.doesNotMatch(markup, /Recorded failures/);
  assert.throws(() => validateFailureContext(context, 'v2:6'));
  for (const mapIndices of [[-1], [1.5], ['1']]) {
    assert.equal(
      validFailureContext({
        verifiers: [
          { message: 'detail', execution: { node: 'assessor', execution: 'id', mapIndices } },
        ],
      }),
      false
    );
  }
  assert.equal(
    validFailureContext({ exhaustedLoops: [{ node: 'refinement', completedIterations: 0 }] }),
    false
  );
  assert.equal(validFailureContext({ verifiers: Array(6).fill({ message: 'detail' }) }), false);
});
