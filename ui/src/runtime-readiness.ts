import { allNodes, bindingFor, executable, type Document } from './domain';

/** A runtime setting the profile still needs before the server can check it. */
export type MissingRuntimeSetting = { field: string; message: string };

/**
 * Finds the first empty runtime choice, in the order Run settings shows them. Only emptiness is
 * checked here; which harnesses, providers and models are valid stays with the server.
 */
export function missingRuntimeSetting(doc: Document): MissingRuntimeSetting | undefined {
  if (!doc.runtime.harness) return { field: 'runtime.harness', message: 'Choose a harness.' };
  if (!doc.runtime.provider) return { field: 'runtime.provider', message: 'Choose a provider.' };
  const unset = allNodes(doc.graph.root)
    .filter(executable)
    .find((node) => {
      const binding = bindingFor(doc.runtime, node.name);
      return binding?.kind === 'agent' && !binding.model;
    });
  return unset
    ? {
        field: `runtime.nodes.${unset.name}.model`,
        message: `Choose a model for \`${unset.name}\`.`,
      }
    : undefined;
}
