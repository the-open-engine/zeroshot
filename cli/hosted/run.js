'use strict';

const DEFAULT_HOSTED_TRANSPORT = 'queue';
const TRANSPORT_LOADERS = Object.freeze({
  direct: () => require('./direct-transport'),
  queue: () => require('./queue-transport'),
});

function selectHostedTransport(name = DEFAULT_HOSTED_TRANSPORT) {
  const load = TRANSPORT_LOADERS[name];
  if (!load) throw new Error(`unknown hosted transport: ${name}`);
  return load();
}

const activeTransport = selectHostedTransport();

module.exports = {
  DEFAULT_HOSTED_TRANSPORT,
  cancelHostedRun: activeTransport.cancel,
  runHosted: activeTransport.run,
  selectHostedTransport,
  statusHostedRun: activeTransport.status,
};
