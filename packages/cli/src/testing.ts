import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** Test helper: a repo dir with a ratchet.yaml and a fake bench that prints successive values. */
export function makeRepo(opts: {
  values: number[];
  metric?: string;
  benchRun?: string;
  yamlOverrides?: string;
  /** JSON template for bench output; `$V` is replaced by the value. Default `{"lat":{"p95":$V}}`. */
  template?: string;
}): { dir: string; config: string } {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-cli-'));
  const template = opts.template ?? '{"lat":{"p95":$V}}';
  writeFileSync(
    join(dir, 'bench.mjs'),
    `import { existsSync, readFileSync, writeFileSync } from 'node:fs';
const values = ${JSON.stringify(opts.values)};
const f = 'counter.txt';
const i = existsSync(f) ? Number(readFileSync(f, 'utf8')) : 0;
writeFileSync(f, String(i + 1));
console.log('warming up {not json}');
console.log(${JSON.stringify(template)}.replace('$V', String(values[i % values.length])));
`,
  );
  const config = join(dir, 'ratchet.yaml');
  writeFileSync(
    config,
    `version: 0
objective:
  metric: ${opts.metric ?? 'lat.p95'}
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: ${opts.benchRun ?? 'node bench.mjs'}
  repeats: 3
  hardware: c7i.2xlarge
guardrails:
  - run: pytest -q
  - error_rate: "< 0.001"
protected: [bench/**, ratchet.yaml]
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge]
${opts.yamlOverrides ?? ''}`,
  );
  return { dir, config };
}
