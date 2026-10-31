// Generates the golden fixtures in this directory from the TypeScript implementation (the executable spec).
//
// The Rust tests (`tests/golden.rs`) read the JSON files and assert that `ratchet-core` produces exactly the
// same answers: same numbers bit for bit, same strings, same errors. Regenerate only when the TypeScript
// behaviour is meant to change:
//
//   cd packages/core && RATCHET_TS_SRC=$PWD/src ../../node_modules/.bin/tsx \
//     ../../crates/ratchet-core/tests/golden/generate.mjs
//
// (`tsx` and the packages' node_modules come from `pnpm install` at the repository root.) Inputs come from a
// seeded generator, so a re-run produces identical files.
import { createRequire } from 'node:module';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const src = process.env.RATCHET_TS_SRC;
if (!src) throw new Error('set RATCHET_TS_SRC to packages/core/src');
const require = createRequire(join(src, '..', 'package.json'));
const YAML = require('yaml');
const core = await import(pathToFileURL(join(src, 'index.ts')).href);

// ------------------------------------------------------------------------------------------------ random
function mulberry32(a) {
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rnd = mulberry32(20260101);
const pick = (xs) => xs[Math.floor(rnd() * xs.length)];
const int = (lo, hi) => lo + Math.floor(rnd() * (hi - lo + 1));
const write = (name, value) => writeFileSync(join(here, name), JSON.stringify(value) + '\n');
/** JSON cannot carry Infinity / NaN: mark them. */
const num = (x) => (Number.isFinite(x) ? x : String(x));

// ------------------------------------------------------------------------------------------------ numfmt
{
  const values = [];
  const mags = [1e-9, 1e-7, 1e-6, 1e-4, 1e-3, 0.01, 0.1, 1, 10, 100, 1e3, 1e4, 1e5, 1e6, 1e7, 1e9, 1e12, 1e15, 1e20];
  for (let i = 0; i < 600; i++) values.push(pick(mags) * rnd() * pick([1, 1, 2, 5, 9.999]));
  // dyadic values: exact ties for toFixed / toPrecision
  for (let i = 0; i < 400; i++) values.push(int(1, 4000) / pick([2, 4, 8, 16, 32, 64, 1024]));
  for (const k of [0.5, 1.5, 2.5, 0.25, 0.75, 0.125, 0.375, 1234565, 1234575, 999999.5, 99999.95, 0.015, 0.035, 1.005, 8.345, 10.235, 0.0001, 0, 1e21, 123456789012345680000, 5e-324, 1.7976931348623157e308])
    values.push(k);
  const out = { toFixed: [], toPrecision: [], toString: [] };
  for (const x of values) {
    for (const d of [0, 1, 2, 6]) out.toFixed.push([x, d, x >= 1e21 ? null : x.toFixed(d)]);
    out.toPrecision.push([x, 6, x.toPrecision(6)]);
    out.toString.push([x, String(x)]);
    out.toString.push([-x, String(-x)]);
  }
  out.toFixed = out.toFixed.filter((r) => r[2] !== null);
  write('numfmt.json', out);
}

// ------------------------------------------------------------------------------------------------ stats + calibrate
function sampleSet() {
  const n = pick([1, 2, 3, 3, 4, 5, 5, 7, 10]);
  const base = pick([0, 0.01, 1, 3.5, 42, 100, 1000, -50, 1e6]);
  const spread = pick([0, 0.001, 0.01, 0.05, 0.2, 1]);
  return Array.from({ length: n }, () => base + (rnd() - 0.5) * 2 * spread * (base === 0 ? 1 : Math.abs(base)));
}
{
  const cases = [];
  for (let i = 0; i < 400; i++) {
    const samples = sampleSet();
    const s = core.summarize(samples);
    const c = { samples, summary: { n: s.n, mean: s.mean, median: s.median, stddev: s.stddev, cv: num(s.cv), min: s.min, max: s.max }, noise: [] };
    for (const opts of [{}, { k: 3 }, { minAbsolute: 0.5 }, { k: 0.5, minAbsolute: 2 }]) c.noise.push([opts, core.noiseFloor(samples, opts)]);
    cases.push(c);
  }
  write('stats.json', cases);

  const cal = [];
  for (let i = 0; i < 600; i++) {
    const samples = sampleSet();
    const input = { samples };
    if (rnd() < 0.3) input.repeats = int(1, 20);
    if (rnd() < 0.3) input.minGain = pick([0, 0.005, 0.01, 0.03, 0.05, 0.1, 0.5, 1]);
    if (rnd() < 0.2) input.noiseFactor = pick([0, 1, 2, 3, 10]);
    cal.push({ input, out: core.calibrate(input) });
  }
  // sweep of CVs, as the unit tests do
  for (const cv of [0.0005, 0.001, 0.002, 0.006, 0.01, 0.015, 0.02, 0.03, 0.043, 0.05, 0.08, 0.1, 0.2, 0.3, 0.5, 1, 3]) {
    for (const n of [3, 4, 5, 6, 9]) {
      const raw = Array.from({ length: n }, (_, i) => (i % 2 === 0 ? 1 : -1));
      if (n % 2 === 1) raw[n - 1] = 0;
      const mean = raw.reduce((a, b) => a + b, 0) / n;
      const sd = Math.sqrt(raw.reduce((a, b) => a + (b - mean) ** 2, 0) / (n - 1));
      const samples = raw.map((r) => 100 + (r / sd) * 100 * cv);
      for (const extra of [{}, { minGain: 0.03 }, { repeats: 3 }, { minGain: 0.03, repeats: 5 }]) {
        const input = { samples, ...extra };
        cal.push({ input, out: core.calibrate(input) });
      }
    }
  }
  write('calibrate.json', cal);
  const nice = [];
  for (let i = 0; i < 600; i++) {
    const x = pick([rnd() * 0.05, rnd() * 0.3, rnd() * 2, 0.005 * int(0, 60), 0.01 * int(0, 40)]);
    nice.push([x, core.niceGain(x), core.formatPercent(x)]);
  }
  write('nice_gain.json', nice);
}

// ------------------------------------------------------------------------------------------------ ratchet rule
{
  const cases = [];
  for (let i = 0; i < 1000; i++) {
    const best = pick([100, 100, 0.5, 1234.5678, -100, 1e6, 3]);
    const scale = Math.abs(best) || 1;
    const input = {
      direction: pick(['minimize', 'maximize']),
      minGain: pick([0, 0.01, 0.03, 0.1, 0.5]),
      noiseFloor: pick([0, 0.1, 1, 8, 0.01 * scale]),
      currentBest: best,
      candidate: Array.from({ length: int(1, 5) }, () => best + (rnd() - 0.5) * 0.3 * scale),
      guardrails: [],
      protectedViolations: [],
    };
    if (rnd() < 0.6) input.confirm = Array.from({ length: int(0, 4) }, () => best + (rnd() - 0.5) * 0.3 * scale);
    if (rnd() < 0.2) input.confirmBest = best * pick([0.5, 1, 5]);
    if (rnd() < 0.15) input.guardrails.push({ name: pick(['pytest -q', 'error_rate < 0.001']), passed: false, ...(rnd() < 0.5 ? { detail: 'exited with status 1' } : {}) });
    if (rnd() < 0.1) input.guardrails.push({ name: 'ok', passed: true });
    if (rnd() < 0.1) input.protectedViolations.push({ path: 'bench/x.js', pattern: 'bench/**' });
    cases.push({ input, out: core.evaluateRatchet(input) });
  }
  // ties and exact thresholds
  for (const dir of ['minimize', 'maximize']) {
    for (const d of [2, 2.9999999, 3, 3.0000001, 3.1, 10]) {
      const input = { direction: dir, minGain: 0.03, noiseFloor: 1, currentBest: 100, candidate: [dir === 'minimize' ? 100 - d : 100 + d], guardrails: [], protectedViolations: [] };
      cases.push({ input, out: core.evaluateRatchet(input) });
    }
  }
  write('rule.json', cases);
}

// ------------------------------------------------------------------------------------------------ globs
{
  const atoms = ['a', 'b', 'x', 'ab', 'bench', 'dir', '.', '-', '_', 'é', '\u{1F600}'];
  const pathOf = () => {
    const segs = Array.from({ length: int(1, 4) }, () => pick(atoms) + (rnd() < 0.3 ? pick(atoms) : ''));
    let p = segs.join('/');
    if (rnd() < 0.1) p = './' + p;
    if (rnd() < 0.05) p = '/' + p;
    if (rnd() < 0.05) p = p.replace('/', '\\');
    if (rnd() < 0.05) p += '/';
    if (rnd() < 0.05) p = 'x/../' + p;
    return p;
  };
  const gAtoms = ['a', 'b', 'x', 'ab', 'bench', 'dir', '*', '**', '**/', '?', '[ab]', '[!a]', '[^b]', '[a-c]', '{a,b}', '{ab,x}', '{a,}', '/', '.', '-', '+', '(', '$'];
  const globOf = () => {
    const n = int(1, 5);
    let g = '';
    for (let i = 0; i < n; i++) g += pick(gAtoms);
    if (rnd() < 0.05) g = './' + g;
    if (rnd() < 0.05) g += '/';
    return g;
  };
  const weird = ['[]', '[!]', '[]a]', '[!]a]', '{', '}', 'a{', 'a}', '{a', 'a{b}c}', '[', 'a[', '[a', '[a]b]', '**', '*', '', '/', '.', 'a[!]b', '[a-', '[-a]', '[a-]', '{a,b,c}d', '{*,b}', 'a**b', '**a', 'a**', '***', '**/**', '*/', '?/?', '[z-a]', 'x{a[}]', '[b-a]c'];
  const pairs = [];
  const normalize = [];
  const globs = [...weird, ...Array.from({ length: 350 }, globOf)];
  for (const g of globs) {
    for (let k = 0; k < 6; k++) {
      const p = rnd() < 0.2 ? pick(['a', 'b/a', 'a/b', 'bench/a', 'ab', 'x/dir/b', '.a', 'a.b']) : pathOf();
      try {
        pairs.push([p, g, core.matchesGlob(p, g)]);
      } catch {
        pairs.push([p, g, null]); // the TypeScript threw (invalid regular expression)
      }
    }
  }
  for (let i = 0; i < 800; i++) {
    const p = pathOf();
    normalize.push([p, core.normalizeRepoPath(p)]);
  }
  for (const p of ['', '.', './', '..', '../a', 'a/..', 'a/../..', '/..', '/', '//', 'a//b', './/a', './/./a', 'a/./b/.', '\\', 'a\\..\\b', '...', 'a/.../b']) normalize.push([p, core.normalizeRepoPath(p)]);
  const violations = [];
  for (let i = 0; i < 300; i++) {
    const gl = Array.from({ length: int(0, 4) }, globOf).filter((g) => {
      try {
        core.matchesGlob('a', g);
        return true;
      } catch {
        return false;
      }
    });
    const paths = Array.from({ length: int(0, 6) }, pathOf);
    violations.push({ paths, globs: gl, out: core.findProtectedViolations(paths, gl) });
  }
  write('glob.json', { pairs, normalize, violations });
}

// ------------------------------------------------------------------------------------------------ protected set
{
  const cmds = [
    'k6 run bench/k6/search.js', 'node eval/backtest.mjs', 'node ./bench/run.mjs && node eval/check.mjs', 'node dist/bench/hyperfine.js --runs 10',
    'node dist/bench/hyperfine.js --mix "${RATCHET_TRACE:-bench/invocations.json}"', 'node benchmark/run.js', 'node my-evaluation/run.js', 'node /opt/bench/run.js',
    'node ../bench/run.js', 'make eval', 'sh ./score.sh', 'cd sub && node bench/x.js', 'a/b/c/bench/x.js eval/y.js', './a/./bench/x', 'x@y/bench/z', '.bench/x', 'a.bench/x',
    'node ./a/../bench/run.js', 'node a/../../bench/run.js', 'bench/ eval/', 'bench', 'eval/', 'node -e "require(\'./bench/x\')"', '--config=bench/c.json eval/d.js',
    'FOO=bench/a node eval/b', 'pytest bench/ -k eval/x', 'a-b_c/bench/d e.f/eval/g', '"bench/x"', "'./eval/y'", '(bench/x)', 'bench/bench/bench/x', 'dist/eval/eval/x',
  ];
  const words = ['bench', 'eval', 'node', 'dist', 'src', '.', '..', 'a', 'b.c', 'x-y', '@s', ' ', '/', '/', '/', './', 'run.js', '&&', '"', "'", '${X:-', '}'];
  for (let i = 0; i < 300; i++) cmds.push(Array.from({ length: int(2, 9) }, () => pick(words)).join(rnd() < 0.5 ? '' : ' '));
  const runs = cmds.map((c) => [c, core.evaluatorDirsFromRun(c)]);
  const tpls = [undefined, 'http-latency', 'forecast-backtest', 'llm-eval', 'nope', 'memory'];
  const configs = [];
  for (let i = 0; i < 120; i++) {
    const input = { evaluator: {} };
    if (rnd() < 0.7) input.evaluator.run = pick(cmds);
    const t = pick(tpls);
    if (t) input.evaluator.template = t;
    if (rnd() < 0.4) input.environment = { image: pick(['.devcontainer/Dockerfile', 'Dockerfile.bench', '.devcontainer/devcontainer.json', './x/Dockerfile', '']) };
    if (rnd() < 0.3) input.infra = pick(['ops/tofu', './ops/', 'a/b']);
    if (rnd() < 0.5) input.protected = Array.from({ length: int(0, 3) }, () => pick(['tests/**', 'ratchet.yaml', 'bench/**', './x', '/abs', 'a\\b', '', 'src/*.ts']));
    const configPath = rnd() < 0.3 ? pick(['ops/ratchet.yaml', './ratchet.yaml', 'ratchet.yml', '/x/ratchet.yaml', '']) : undefined;
    configs.push({ input, configPath: configPath ?? null, out: core.effectiveProtected(input, configPath === undefined ? {} : { configPath }) });
  }
  // The approved drafted environment is locked once environment.scope is set (hand-picked, so the seeded draws above stay put).
  for (const environment of [{ scope: 'none' }, { scope: 'single' }, { scope: 'full' }, { scope: 'full', image: '.devcontainer/Dockerfile' }, { scope: 'single', image: 'infra/ratchet/Dockerfile' }, { image: 'Dockerfile' }, {}]) {
    for (const infra of [undefined, 'ops/tofu']) {
      const input = { evaluator: { run: 'node bench/run.js' }, environment, ...(infra ? { infra } : {}), protected: ['tests/**'] };
      configs.push({ input, configPath: null, out: core.effectiveProtected(input, {}) });
    }
  }
  write('protected_set.json', { runs, configs });
}

// ------------------------------------------------------------------------------------------------ guardrails
{
  const raws = ['< 0.001', '<=5', '>= 0.99', '> 10', '== 0', '< 1.2x', '  <  2 x ', '< 1e-3', '< .5', '> -1', '', '0.001', '< ', '<< 1', '< abc', '< 1.2y', '< 1 2', '!= 3', '< 0x', '< -0x', '< 1e', '< 1.e2', '< 5.', '< +3', '< 1e999', '< 1.2.3', '< .', '<1', '<=-1.5e+3x', '< 0.0x', '\t< 1\n', '< 1 x', '< 1X', '< 12abc'];
  const parsed = raws.map((raw) => {
    const r = core.parseThreshold('m', raw);
    return [raw, r];
  });
  const evals = [];
  for (let i = 0; i < 400; i++) {
    const raw = pick(raws.filter((r) => typeof core.parseThreshold('m', r) !== 'string'));
    const g = core.parseThreshold('m', raw);
    const observed = pick([0, 0.0005, 0.001, 1, 110, 121, -3, 1e-7, 1e22, NaN, Infinity, 0.1 + 0.2]);
    const baseline = pick([undefined, undefined, 100, 0, -5, NaN, 1e-7]);
    const out = core.evaluateThreshold(g, observed, baseline);
    evals.push({ g, observed: num(observed), baseline: baseline === undefined ? null : num(baseline), out });
  }
  write('guardrails.json', { parsed, evals });
}

// ------------------------------------------------------------------------------------------------ templates
{
  const ids = core.listTemplates({ kind: 'template' }).map((t) => t.id);
  const rendered = {};
  const optionSets = [{}, { metric: 'a: b', direction: 'maximize', valueUsd: 7 }, { metric: '  spaced.metric  ', valueUsd: 1e21 }, { valueUsd: 0.0000001, metric: '' }];
  for (const id of [...ids, 'perf', 'forecasting']) {
    rendered[id] = optionSets.map((o) => {
      const r = core.renderTemplate(id, o);
      const p = core.buildTemplateProposal(id, o);
      return { options: o, resolved: r.options, files: r.files, proposal: { branch: p.branch, commitMessage: p.commitMessage, title: p.title, body: p.body } };
    });
  }
  write('templates.json', { list: core.listTemplates(), rendered });
}

// ------------------------------------------------------------------------------------------------ config
{
  const VALID = `
version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: k6 run bench/k6/search.js
  repeats: 5
  hardware: c7i.2xlarge
  confirm_trace: s3://sealed/search-trace.jsonl
guardrails:
  - run: pytest -q
  - run: openapi-diff --base main
  - error_rate: "< 0.001"
  - peak_rss: "< 1.2x"
protected:
  - bench/**
  - ratchet.yaml
  - tests/contract/**
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge, c7i.xlarge, m7i.large]
`;
  const GENERIC = `
version: 0
pack: forecasting
objective:
  name: Weekly sales forecast
  metric: wape
  direction: minimize
  min_gain: 0.02
  value: { usd_per_unit: 250000, period: year, unit: "WAPE point", note: n }
evaluator:
  kind: backtest
  ref: ratchet/eval
  run: python -m eval.backtest
  repeats: 3
  noise_factor: 3
  hardware: m7i.xlarge
  deps: pnpm
  holdout: s3://sealed/last-8-weeks.parquet
environment:
  image: .devcontainer/Dockerfile
  machine: c7i.2xlarge
  region: eu-west-1
  network: [registry.npmjs.org, "*.pythonhosted.org"]
infra: ops/sandbox
agent: { instructions: RATCHET.md, allowed_instance_types: [m7i.xlarge] }
`;
  const MINIMAL = `version: 0
objective: { metric: http_req_duration.p95, direction: minimize }
evaluator:
  template: http-latency
`;
  const bases = [VALID, GENERIC, MINIMAL].map((t) => YAML.parse(t));
  const bad = [null, 0, -1, 1, 1.5, 3, 11, '', ' ', 'x', 'Bad_Id', '< 1', true, false, [], ['a'], [5], {}, { a: 1 }, 'c7i.large', '../x', '/x', 'a\\b', 'us-east-1', 'registry.npmjs.org', 'nope', 'demo-search', 'cli-cold-start', 'pnpm', 'yarn', 'maximize', 0.5];
  const cases = [];
  const add = (doc) => {
    const text = YAML.stringify(doc);
    cases.push({ yaml: text, result: core.parseConfig(text) });
  };
  const paths = (o, prefix = []) => {
    const out = [];
    if (o && typeof o === 'object') {
      for (const k of Object.keys(o)) {
        out.push([...prefix, k]);
        out.push(...paths(o[k], [...prefix, k]));
      }
    }
    return out;
  };
  const setAt = (doc, path, value) => {
    const d = structuredClone(doc);
    let cur = d;
    for (const k of path.slice(0, -1)) cur = cur[k];
    if (value === undefined) delete cur[path.at(-1)];
    else cur[path.at(-1)] = value;
    return d;
  };
  for (const base of bases) {
    add(base);
    for (const p of paths(base)) {
      add(setAt(base, p, undefined)); // remove
      for (const v of bad) if (rnd() < 0.22) add(setAt(base, p, v));
      // a typo key next to it
      const parent = p.slice(0, -1);
      add(setAt(base, [...parent, 'zzz'], 1));
    }
    // guardrail shapes
    for (const g of [[{ run: 'a', error_rate: '< 1' }], [{}], ['x'], [{ run: '' }], [{ run: '  ' }], [{ m: '< abc' }], [{ m: 1 }], [{ m: '< 1x' }, { run: 'z' }], [{ m: true }], [[]], [null], 'not a list', {}]) add({ ...base, guardrails: g });
    // template ids
    for (const t of ['http-latency', 'memory', 'nope', 'demo-cli', 'A', '1a', 'a b', 'a--b', 'llm-eval', 5, null]) add({ ...base, evaluator: { ...(base.evaluator ?? base.bench ?? {}), template: t } });
    // bench alias
    if (base.evaluator) add({ ...structuredClone(base), bench: structuredClone(base.evaluator) });
  }
  for (const text of ['', '   ', '# nothing', 'null', '~', '[]', '- a', 'just a string', '42', 'version: 0\n', 'version: 1\n', 'version: 0.0\nobjective: 1\nevaluator: 2\n', 'a: b: c', 'a: [', "a: 'x", '\tversion: 0', 'version: 0\nversion: 0\n']) cases.push({ yaml: text, result: core.parseConfig(text) });
  // environment.scope (hand-picked, appended so the seeded draws above stay put)
  const scopeBase = 'version: 0\nobjective:\n  metric: m\n  direction: minimize\nevaluator:\n  run: node bench/x.js\n';
  for (const env of ['scope: none', 'scope: single', 'scope: full', 'scope: huge', 'scope: 3', 'scope: [full]', 'scope: null', 'scope: ""', 'scope: full\n  machine: c7i.2xlarge\n  image: infra/ratchet/Dockerfile', 'scope: FULL', 'scope: Full\n  zzz: 1']) {
    const yaml = `${scopeBase}environment:\n  ${env}\n`;
    cases.push({ yaml, result: core.parseConfig(yaml) });
  }
  write('config.json', cases);
}
console.log('golden fixtures written to', here);
