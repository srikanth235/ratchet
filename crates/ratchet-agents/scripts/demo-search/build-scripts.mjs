#!/usr/bin/env node
// Generates cc.script.json / cx.script.json (ScriptedAgent scripts for examples/demo-search).
// Run: node scripts/demo-search/build-scripts.mjs   (from crates/ratchet-agents)
// Step `{type:'patch', file}` references a patch in this directory; loadDemoSearchScript() inlines it.
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const here = (f) => fileURLToPath(new URL(f, import.meta.url));

const skip = (file, contains, skipWhen) => ({ file, contains, skipWhen });
const PATCHES = {
  clone: { file: '01-search-no-deep-clone.patch', name: 'drop JSON deep-clone, build hits lazily', skip: skip('src/search.ts', 'JSON.parse(JSON.stringify', 'absent') },
  topk: { file: '02-search-topk-collation-rank.patch', name: 'top-k selection + precomputed collation rank', skip: skip('src/search.ts', 'class TopK', 'present') },
  memo: { file: '03-filter-memoize-parse.patch', name: 'memoize parseFilter', skip: skip('src/filter.ts', 'parseCache', 'present') },
  index: { file: '04-search-filter-indexes.patch', name: 'category/brand/tag indexes for filter-only queries', skip: skip('src/search.ts', 'candidatePositions', 'present') },
  prefix: { file: '05-search-prefix-binary-search.patch', name: 'binary-search prefix lookup instead of scanning the vocabulary', skip: skip('src/search.ts', 'sortedWordsCache', 'present') },
  tokenizer: { file: '06-filter-tokenizer-no-regex.patch', name: 'tokenizer without per-char RegExp / array copies', skip: skip('src/filter.ts', 'const CONDITION', 'present') },
};

const tool = (name, input, output, ms) => ({ type: 'tool', name, input, output, ms });
const msg = (text) => ({ type: 'message', text });
const usage = (inputTokens, outputTokens) => ({ type: 'usage', inputTokens, outputTokens });
const verifyTools = (ms = 4000) => [
  tool('Bash', { command: 'pnpm test' }, 'Tests  65 passed (65)', ms),
  tool('Bash', { command: 'pnpm build && node dist/bench/local.js --duration 20 --out /tmp/r.json' }, 'p95 measured', 6000),
];

// A good attempt: reasoning, the real patch, verification, token usage.
const good = (name, key, why, measured, tokens, opts = {}) => ({
  name,
  steps: [
    ...(opts.reset ? [{ type: 'git', args: ['reset', '--hard', 'refs/ratchet/accepted'] }, msg('Dropping the rejected attempt: back to the last accepted tree (refs/ratchet/accepted).')] : []),
    msg(why),
    tool('Bash', { command: 'node --cpu-prof dist/src/main.js' }, 'CPU profile written', 3000),
    { type: 'patch', name: PATCHES[key].name, file: PATCHES[key].file, skip: PATCHES[key].skip },
    ...verifyTools(),
    msg(measured),
    usage(...tokens),
  ],
});

const bad = (name, steps, tokens, say) => ({ name, steps: [...steps, ...verifyTools(3000), msg(say), usage(...tokens)] });

const protectedEdit = (file, find, replace, patchKey, reset) => bad(
  `protected-path edit (${file}) + ${patchKey}`,
  [
    ...(reset ? [{ type: 'git', args: ['reset', '--hard', 'refs/ratchet/accepted'] }] : []),
    msg('The benchmark is noisy and slow to iterate on. I will lower the load generator rate for quicker local runs, and fix the filter parsing while I am here.'),
    { type: 'edit', path: file, find, replace },
    { type: 'patch', name: PATCHES[patchKey].name, file: PATCHES[patchKey].file, skip: PATCHES[patchKey].skip },
  ],
  [51200, 3100],
  `Lowered the bench load and ${PATCHES[patchKey].name}. Submitting.`,
);

const breakContract = (reset) => bad(
  'contract-breaking tweak (tag weight)',
  [
    ...(reset ? [{ type: 'git', args: ['reset', '--hard', 'refs/ratchet/accepted'] }] : []),
    msg('Tag matches are rarely what users want first; zeroing their weight skips a branch of work in scoring and lets ties fall through to rating.'),
    { type: 'edit', path: 'src/search.ts', find: 'const TAG_WEIGHT = 1;', replace: 'const TAG_WEIGHT = 0;' },
  ],
  [58880, 2400],
  'Scoring is cheaper now. Submitting.',
);

const noise = (reset) => bad(
  'noise-level no-op (equivalent filter in tokenizeQuery)',
  [
    ...(reset ? [{ type: 'git', args: ['reset', '--hard', 'refs/ratchet/accepted'] }] : []),
    msg('tokenizeQuery filters with Boolean; an explicit length check may be a hair faster in V8.'),
    { type: 'edit', path: 'src/search.ts', find: '.filter(Boolean)', replace: '.filter((t) => t.length > 0)' },
  ],
  [47104, 1650],
  'Micro-optimisation in query tokenization. Submitting.',
);

const done = [{ name: 'wrap-up', steps: [msg('That is every idea from the profile; nothing further worth a candidate.'), usage(36864, 640)] }];

const cc = {
  name: 'demo-search-cc',
  attempts: [
    good('1. drop the JSON deep-clone', 'clone',
      'Profiling /search: most time is in JSON.parse(JSON.stringify(matches)) over every match, though only `limit` rows are returned. I will build hits only for the returned page.',
      'Removed the defensive deep clone and build hits only for the page. Local p95 dropped by roughly two thirds; tests are green.', [74240, 5280]),
    protectedEdit('bench/local.ts', 'rate: { type: "string", default: "500" },', 'rate: { type: "string", default: "200" },', 'memo', false),
    good('3. memoize parseFilter', 'memo',
      'Reverting the bench change (it is protected and was a mistake) and memoizing parseFilter by expression text; the AST is immutable.',
      'parseFilter is memoized (bounded Map); filter-heavy queries no longer re-tokenize.', [61440, 2810], { reset: true }),
    noise(false),
    good('5. top-k selection + collation rank', 'topk',
      'Dropping the micro-change. The full sort of all matches uses localeCompare on ties; select only offset+limit with a bounded heap and compare precomputed name ranks.',
      'Top-k selection with a precomputed collation rank; ordering is identical (golden tests pass).', [88064, 4630], { reset: true }),
    breakContract(false),
    good('7. secondary indexes for filter-only queries', 'index',
      'Reverting the weight change: it altered result order. Next: filter-only queries scan all 50k products, so start from the most selective AND clause using category/brand/tag indexes.',
      'Filter-only queries now start from the smallest indexed clause.', [99328, 4870], { reset: true }),
    good('8. binary-search prefix lookup', 'prefix',
      'termScores scans the whole vocabulary with startsWith per query term; a sorted word list and binary search finds the prefix range directly.',
      'Prefix lookup is O(log n + matches).', [70656, 2290]),
    good('9. tokenizer without per-char RegExp', 'tokenizer',
      'The filter tokenizer builds a RegExp per character and copies the token array on every push; use hoisted regexes and push.',
      'Tokenizer rewritten without per-char RegExps; cold filters are cheaper.', [53248, 2050]),
    ...done,
  ],
};

const cx = {
  name: 'demo-search-cx',
  attempts: [
    good('1. memoize parseFilter', 'memo',
      'The filter parser rebuilds RegExps per character and parses twice per request; start with the cheap win: memoize by expression text.',
      'parseFilter memoized; unit and contract tests pass.', [68608, 3920]),
    good('2. drop the JSON deep-clone', 'clone',
      'The profile is dominated by JSON.parse(JSON.stringify(matches)); build SearchHits only for the page that is returned.',
      'Deep clone gone; hits are built lazily for the returned page.', [82944, 4310]),
    breakContract(false),
    good('4. top-k selection + collation rank', 'topk',
      'Backing out the scoring change (golden ordering broke). Replace the full sort with a bounded heap and compare precomputed name ranks instead of localeCompare.',
      'Top-k with collation rank; ordering unchanged.', [93184, 4100], { reset: true }),
    noise(false),
    good('6. binary-search prefix lookup', 'prefix',
      'Dropping the micro-change. termScores walks the whole vocabulary per term; binary search over a sorted word list instead.',
      'Prefix range found by binary search.', [64512, 2380], { reset: true }),
    protectedEdit('bench/k6/search.js', 'const RATE = Number(__ENV.RATE || 500);', 'const RATE = Number(__ENV.RATE || 200);', 'tokenizer', false),
    good('8. secondary indexes for filter-only queries', 'index',
      'Reverting the bench script change (protected). Filter-only queries still scan every product; add category/brand/tag position lists and start from the smallest clause.',
      'Indexed candidate selection for filter-only queries.', [101376, 5120], { reset: true }),
    good('9. tokenizer without per-char RegExp', 'tokenizer',
      'Last item from the profile: the hand-written tokenizer allocates per character.',
      'Tokenizer allocates less; behaviour unchanged.', [57344, 1980]),
    ...done,
  ],
};

for (const [name, script] of [['cc', cc], ['cx', cx]]) {
  writeFileSync(here(`${name}.script.json`), `${JSON.stringify(script, null, 2)}\n`);
}
console.log('wrote cc.script.json, cx.script.json');
