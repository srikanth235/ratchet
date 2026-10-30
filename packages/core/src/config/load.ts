import { readFileSync } from 'node:fs';
import { parseDocument } from 'yaml';
import { configSchema, type RatchetConfig } from './schema.js';

export interface ConfigError {
  /** YAML path, e.g. `guardrails[2].peak_rss` or `objective.direction`. Empty for whole-document errors. */
  path: string;
  message: string;
}

export type ConfigResult =
  | { ok: true; config: RatchetConfig }
  | { ok: false; errors: ConfigError[] };

/** Render a path of keys/indices as `a.b[0].c`. */
export function formatPath(path: ReadonlyArray<PropertyKey>): string {
  let out = '';
  for (const seg of path) {
    if (typeof seg === 'number') out += `[${seg}]`;
    else out += out ? `.${String(seg)}` : String(seg);
  }
  return out;
}

/** Format an error as a single human-readable line. */
export function formatConfigError(e: ConfigError): string {
  return e.path ? `${e.path}: ${e.message}` : e.message;
}

/** Parse and validate ratchet.yaml text. Never throws for bad input. */
export function parseConfig(text: string): ConfigResult {
  const doc = parseDocument(text, { prettyErrors: false });
  if (doc.errors.length > 0) {
    return {
      ok: false,
      errors: doc.errors.map((e) => {
        const line = e.linePos?.[0];
        return {
          path: '',
          message: `invalid YAML${line ? ` (line ${line.line}, column ${line.col})` : ''}: ${e.message.split('\n')[0]}`,
        };
      }),
    };
  }
  const value: unknown = doc.toJS();
  if (value === null || value === undefined) {
    return { ok: false, errors: [{ path: '', message: 'ratchet.yaml is empty' }] };
  }
  if (typeof value !== 'object' || Array.isArray(value)) {
    return { ok: false, errors: [{ path: '', message: 'ratchet.yaml must be a mapping at the top level' }] };
  }
  const res = configSchema.safeParse(value);
  if (res.success) return { ok: true, config: res.data };
  return {
    ok: false,
    errors: res.error.issues.map((i) => ({ path: formatPath(i.path), message: describeIssue(i) })),
  };
}

type Issue = { code: string; message: string; path: PropertyKey[]; keys?: string[]; input?: unknown };

function describeIssue(i: Issue): string {
  if (i.code === 'unrecognized_keys' && i.keys) {
    return `unknown ${i.keys.length > 1 ? 'keys' : 'key'} ${i.keys.map((k) => JSON.stringify(k)).join(', ')}`;
  }
  if (i.code === 'invalid_type' && i.input === undefined && /expected .* received undefined/i.test(i.message)) {
    return 'is required';
  }
  return i.message;
}

/** Read and validate a ratchet.yaml file. A missing/unreadable file is reported as an error, not thrown. */
export function loadConfig(path: string): ConfigResult {
  let text: string;
  try {
    text = readFileSync(path, 'utf8');
  } catch (err) {
    const code = (err as NodeJS.ErrnoException).code;
    return {
      ok: false,
      errors: [
        {
          path: '',
          message: code === 'ENOENT' ? `cannot find ${path}` : `cannot read ${path}: ${(err as Error).message}`,
        },
      ],
    };
  }
  return parseConfig(text);
}
