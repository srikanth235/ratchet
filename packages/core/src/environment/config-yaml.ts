import { parseDocument } from 'yaml';
import type { EnvironmentScope } from '../config/schema.js';

export interface EnvironmentConfigPatch {
  scope: EnvironmentScope;
  machine?: string;
  image?: string;
  region?: string;
}

/**
 * Set the `environment:` keys of a ratchet.yaml text and nothing else (comments, key order and every other block are kept).
 * Used by the mock drafter and by anything that has to record an environment decision in the repo's config.
 */
export function applyEnvironmentToConfigYaml(text: string, patch: EnvironmentConfigPatch): string {
  const doc = parseDocument(text);
  if (doc.errors.length > 0) throw new Error(`ratchet.yaml is not valid YAML: ${doc.errors[0]!.message.split('\n')[0]}`);
  for (const [k, v] of Object.entries(patch)) {
    if (v === undefined) continue;
    doc.setIn(['environment', k], v);
  }
  return doc.toString();
}

/** `environment.scope` of a ratchet.yaml text, or undefined (also for text that is not valid YAML). */
export function readEnvironmentScope(text: string): EnvironmentScope | undefined {
  const doc = parseDocument(text);
  if (doc.errors.length > 0) return undefined;
  const v = doc.getIn(['environment', 'scope']);
  return v === 'none' || v === 'single' || v === 'full' ? v : undefined;
}
