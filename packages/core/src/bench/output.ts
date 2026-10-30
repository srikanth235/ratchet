/**
 * Helpers for reading a benchmark command's output.
 *
 * Contract: the benchmark prints a JSON object to stdout; if it prints several,
 * the LAST top-level JSON object on stdout wins. Other text around it is
 * ignored. The objective metric is looked up by dotted path (`http_req_duration.p95`).
 */

/** Extract the last top-level, parseable JSON object from arbitrary text, or undefined. */
export function extractLastJsonObject(text: string): Record<string, unknown> | undefined {
  let last: Record<string, unknown> | undefined;
  let i = 0;
  while (i < text.length) {
    if (text[i] !== '{') {
      i++;
      continue;
    }
    const end = findObjectEnd(text, i);
    if (end === -1) {
      i++; // unbalanced; try the next '{'
      continue;
    }
    try {
      const v: unknown = JSON.parse(text.slice(i, end + 1));
      if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
        last = v as Record<string, unknown>;
        i = end + 1;
        continue;
      }
    } catch {
      // not JSON (e.g. a `{` in log text); fall through
    }
    i++;
  }
  return last;
}

/** Index of the `}` matching the `{` at `start`, honouring JSON strings; -1 if unbalanced. */
function findObjectEnd(text: string, start: number): number {
  let depth = 0;
  let inStr = false;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (inStr) {
      if (c === '\\') i++;
      else if (c === '"') inStr = false;
    } else if (c === '"') inStr = true;
    else if (c === '{') depth++;
    else if (c === '}' && --depth === 0) return i;
  }
  return -1;
}

/**
 * Look up a dotted path. Keys that themselves contain dots are handled by
 * trying progressively longer joined keys (so `a.b.c` finds `{"a.b": {"c": 1}}`).
 */
export function lookupPath(obj: unknown, path: string): unknown {
  return walk(obj, path.split('.'), 0);
}

function walk(cur: unknown, parts: string[], from: number): unknown {
  if (from >= parts.length) return cur;
  if (cur === null || typeof cur !== 'object') return undefined;
  const rec = cur as Record<string, unknown>;
  for (let to = from + 1; to <= parts.length; to++) {
    const key = parts.slice(from, to).join('.');
    if (Object.prototype.hasOwnProperty.call(rec, key)) {
      const r = walk(rec[key], parts, to);
      if (r !== undefined) return r;
    }
  }
  return undefined;
}

export type MetricResult = { ok: true; value: number } | { ok: false; error: string };

/** Extract a finite numeric metric from bench stdout. */
export function extractMetric(stdout: string, metricPath: string): MetricResult {
  const obj = extractLastJsonObject(stdout);
  if (!obj) return { ok: false, error: 'no JSON object found on stdout' };
  const v = lookupPath(obj, metricPath);
  if (v === undefined) return { ok: false, error: `metric "${metricPath}" not found in JSON output` };
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    return { ok: false, error: `metric "${metricPath}" is not a finite number (got ${JSON.stringify(v)})` };
  }
  return { ok: true, value: v };
}
