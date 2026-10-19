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
