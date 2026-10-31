import { parse as parseYaml } from 'yaml';
import { DEFAULT_ENVIRONMENT_INFRA_DIR, type EnvironmentScope } from '../config/schema.js';
import { parseConfig, formatConfigError } from '../config/load.js';
import { normalizeRepoPath } from '../protected.js';
import { defaultMachines, estimateMachinesCost, INSTANCE_USD_PER_HOUR, type MachineSpec } from './cost.js';
import { literalNumber, literalString, parseHcl, walkBlocks, type HclBlock } from './hcl.js';
import { ENVIRONMENT_ROLES, type EnvironmentCheck, type EnvironmentCost, type EnvironmentFile, type EnvironmentResource, type EnvironmentRole } from './types.js';

/**
 * The validator every drafted environment goes through, before the user sees it and again when the engine applies it
 * (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"; the contract is infra/contract.md plus infra/modules/sandbox).
 *
 * It is a STATIC check, deliberately an allow-list: the draft may only (a) call the base module Ratchet ships, with a literal
 * map of machines, (b) add a few inert resources (`aws_s3_object` seed data under the environment prefix), and (c) carry scripts
 * and a Dockerfile as plain files. Everything else a setup agent could write (IAM, network, public addresses, provisioners,
 * providers, state backends, imports, reads from this machine) fails a named check. `tofu validate` and `tofu plan` in the
 * user's account are the second line (`plan` check, added by the drafter); this one has no cloud, no tofu binary and no clock.
 */

/** Source of the base module as the draft writes it. Ratchet copies the shipped module there when it applies the environment. */
export const SANDBOX_BASE_MODULE_SOURCE = './ratchet-sandbox';
/** The module block's name; the generated outputs and the validator refer to it. */
export const SANDBOX_BASE_MODULE_NAME = 'sandbox';
/** Key prefix in the artifacts bucket a draft's seed objects must live under. */
export const ENVIRONMENT_OBJECT_PREFIX = 'env/';
/** Files Ratchet generates next to a draft when it materializes it; a draft must not ship them. */
export const RATCHET_GENERATED_FILES = ['ratchet_inputs.tf', 'ratchet_outputs.tf'] as const;

/** Check ids, stable for the UI and the tests. The first four are the headline claims ("isolated, results sealed, ..."). */
export const ENVIRONMENT_CHECK_IDS = [
  'isolated',
  'sealed',
  'same-hardware',
  'teardown',
  'files',
  'syntax',
  'config',
  'scope',
  'base-module',
  'local-access',
  'tagged',
  'cost',
  'plan',
] as const;
export type EnvironmentCheckId = (typeof ENVIRONMENT_CHECK_IDS)[number];

export const MAX_ENVIRONMENT_MACHINES = 8;
export const MAX_ENVIRONMENT_FILE_BYTES = 256 * 1024;
export const MAX_ENVIRONMENT_TOTAL_BYTES = 2 * 1024 * 1024;
export const MAX_VOLUME_GB = 1000;

/** Keys of one `machines` entry (the base module's variable). Anything else is refused: no subnet, group, address or identity. */
export const MACHINE_KEYS = ['role', 'instance_type', 'label', 'detail', 'user_data', 'root_volume_gb', 'data_volume_gb'] as const;

const ALLOWED_RESOURCE_TYPES = new Set(['aws_s3_object', 'random_id']);
const ALLOWED_DATA_TYPES = new Set(['aws_caller_identity', 'aws_region', 'aws_partition']);
const FILE_FUNCTIONS = ['file', 'filebase64', 'templatefile', 'fileexists', 'fileset', 'filemd5', 'filesha1', 'filesha256', 'filesha512', 'filebase64sha256', 'filebase64sha512', 'abspath', 'pathexpand'];

export interface ValidateEnvironmentInput {
  scope: EnvironmentScope;
  /** Everything the draft would commit: `ratchet.yaml` (or `configPath`) and the files under `infra/ratchet/`. */
  files: readonly EnvironmentFile[];
  /** ratchet.yaml as it is on the base ref: proves the draft changed `environment:` and nothing else. */
  originalConfigYaml?: string;
  /** Path of the config in the repo (default `ratchet.yaml`). */
  configPath?: string;
  /** The run's time limit for the cost estimate (default 2 h). */
  timeLimitSeconds?: number;
  /**
   * The files are the environment module only (repo paths under `infra/ratchet/`, no ratchet.yaml): what the engine and the
   * sandbox provider re-check when they APPLY an approved environment. The `config` check is then skipped.
   */
  moduleOnly?: boolean;
  /** Hourly prices (default: the static table). */
  prices?: Readonly<Record<string, number>>;
  /** Pinned bench machine for `none` / `single` without OpenTofu (default c7i.large); read from ratchet.yaml when it is part of the draft. */
  hardware?: string;
}

export interface EnvironmentValidation {
  ok: boolean;
  checks: EnvironmentCheck[];
  /** The machines the draft creates (parsed from the module call; the evaluator / pinned machine for `none` / `single` without .tf). */
  machines: MachineSpec[];
  resources: EnvironmentResource[];
  cost: EnvironmentCost;
  /** Flat list of every failed check's problems. */
  problems: string[];
}

const KB = 1024;
const isTf = (p: string): boolean => p.endsWith('.tf');
const baseName = (p: string): string => p.slice(p.lastIndexOf('/') + 1);

function stableJson(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(stableJson).join(',')}]`;
  if (v && typeof v === 'object') {
    return `{${Object.entries(v as Record<string, unknown>)
      .filter(([, x]) => x !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([k, x]) => `${JSON.stringify(k)}:${stableJson(x)}`)
      .join(',')}}`;
  }
  return JSON.stringify(v) ?? 'null';
}

function pass(id: EnvironmentCheckId, label: string, detail: string): EnvironmentCheck {
  return { id, label, ok: true, detail };
}

function result(id: EnvironmentCheckId, label: string, problems: string[], okDetail: string): EnvironmentCheck {
  if (problems.length === 0) return pass(id, label, okDetail);
  const shown = problems.slice(0, 6);
  const more = problems.length > shown.length ? ` (+${problems.length - shown.length} more)` : '';
  return { id, label, ok: false, detail: `${shown.join('; ')}${more}` };
}

interface TfFile {
  path: string;
  blocks: HclBlock[];
  text: string;
}

/** The literal `machines` map of the module call, as machine specs (problems are pushed to `problems`). */
function readMachines(mod: HclBlock, problems: { same: string[]; sealed: string[]; scope: string[]; module: string[] }): MachineSpec[] {
  const attr = mod.attrs['machines'];
  if (!attr) {
    problems.module.push('the module call has no `machines`');
    return [];
  }
  if (attr.kind !== 'object') {
    problems.same.push('`machines` must be a literal map, not an expression: the hardware has to be readable before anything is created');
    return [];
  }
  const out: MachineSpec[] = [];
  for (const [key, v] of Object.entries(attr.entries)) {
    if (v.kind !== 'object') {
      problems.same.push(`machine "${key}" must be an object`);
      continue;
    }
    for (const k of Object.keys(v.entries)) {
      if (!(MACHINE_KEYS as readonly string[]).includes(k)) {
        problems.sealed.push(`machine "${key}" sets "${k}": machines are placed, addressed and given their identity by the base module (allowed keys: ${MACHINE_KEYS.join(', ')})`);
      }
    }
    const role = literalString(v.entries['role']);
    if (!role || !(ENVIRONMENT_ROLES as readonly string[]).includes(role)) {
      problems.module.push(`machine "${key}" needs a role: ${ENVIRONMENT_ROLES.join(' | ')}`);
    }
    const type = literalString(v.entries['instance_type']);
    if (!type) {
      problems.same.push(`machine "${key}": instance_type must be a literal such as "c7i.2xlarge" (not a variable or expression), so every run gets the same hardware`);
    } else if (!/^[a-z][a-z0-9]*\.[a-z0-9]+$/.test(type)) {
      problems.same.push(`machine "${key}": "${type}" is not an instance type`);
    }
    const label = literalString(v.entries['label']) ?? key;
    const detail = literalString(v.entries['detail']) ?? '';
    const data = literalNumber(v.entries['data_volume_gb']);
    const root = literalNumber(v.entries['root_volume_gb']);
    for (const [n, x] of [['data_volume_gb', data], ['root_volume_gb', root]] as const) {
      if (x !== undefined && (x < 0 || x > MAX_VOLUME_GB)) problems.module.push(`machine "${key}": ${n} must be 0-${MAX_VOLUME_GB}`);
    }
    if (type && role) {
      out.push({
        key,
        name: label,
        role: (ENVIRONMENT_ROLES as readonly string[]).includes(role) ? (role as EnvironmentRole) : 'other',
        machine: type,
        detail,
        ...(data !== undefined ? { dataVolumeGb: data } : {}),
        ...(root !== undefined ? { rootVolumeGb: root } : {}),
      });
    }
  }
  return out;
}

/** Arguments of every call to a file function in `text`, e.g. `file("${path.module}/seed.sh")` -> `"${path.module}/seed.sh"`. */
function fileCalls(text: string): Array<{ fn: string; arg: string }> {
  const out: Array<{ fn: string; arg: string }> = [];
  const re = new RegExp(`(?<![\\w.])(${FILE_FUNCTIONS.join('|')})\\s*\\(`, 'g');
  for (const m of text.matchAll(re)) {
    let i = m.index! + m[0].length;
    let depth = 1;
    let arg = '';
    while (i < text.length && depth > 0) {
      const c = text[i]!;
      if (c === '(') depth++;
      else if (c === ')') depth--;
      if (depth > 0) arg += c;
      i++;
    }
    out.push({ fn: m[1]!, arg: arg.trim() });
  }
  return out;
}

const INTERNET_IN_SCRIPT = [
  /\b(?:curl|wget)\b[^\n]*\bhttps?:\/\//i,
  /\bgit\s+clone\s+https?:\/\//i,
  /\b(?:apt(?:-get)?|yum|dnf|apk)\s+(?:-\S+\s+)*(?:install|update|upgrade)\b/i,
  /\bpip3?\s+install\b/i,
  /\bnpm\s+(?:i|install|ci)\b/i,
  /\bdocker\s+pull\b/i,
  /\bgo\s+(?:get|install)\b/i,
];

/** Validate a draft against the sandbox contract. Pure. */
export function validateEnvironment(input: ValidateEnvironmentInput): EnvironmentValidation {
  const configPath = normalizeRepoPath(input.configPath ?? 'ratchet.yaml') || 'ratchet.yaml';
  const prefix = `${DEFAULT_ENVIRONMENT_INFRA_DIR}/`;
  const checks: EnvironmentCheck[] = [];
  const timeLimitSeconds = input.timeLimitSeconds ?? 2 * 3600;

  // ---- files -------------------------------------------------------------------------------------------------------
  const fileProblems: string[] = [];
  const seen = new Set<string>();
  let total = 0;
  const files: EnvironmentFile[] = [];
  for (const f of input.files) {
    const p = normalizeRepoPath(f.path);
    const bytes = Buffer.byteLength(f.content, 'utf8');
    total += bytes;
    if (!p || f.path.includes('\0') || p.split('/').includes('..') || p.split('/').some((s) => s.toLowerCase() === '.git')) {
      fileProblems.push(`unsafe path ${JSON.stringify(f.path)}`);
      continue;
    }
    if (seen.has(p)) {
      fileProblems.push(`${p} appears twice`);
      continue;
    }
    seen.add(p);
    if (p !== configPath && !p.startsWith(prefix)) {
      fileProblems.push(`${p}: a drafted environment may only change ${configPath} and files under ${prefix}`);
    }
    const name = baseName(p);
    if (
      p.startsWith(prefix) &&
      (name.endsWith('.tf.json') ||
        name.endsWith('.tfvars') ||
        name.endsWith('.tfvars.json') ||
        name.endsWith('.tfstate') ||
        name.startsWith('.terraform') ||
        name === 'override.tf' ||
        name.endsWith('_override.tf') ||
        (RATCHET_GENERATED_FILES as readonly string[]).includes(name) ||
        p.startsWith(`${prefix}ratchet-sandbox/`))
    ) {
      fileProblems.push(`${p}: this file type is not allowed (variable files, overrides, state, JSON configuration and Ratchet's own files are not part of a draft)`);
    }
    if (bytes > MAX_ENVIRONMENT_FILE_BYTES) fileProblems.push(`${p} is ${Math.round(bytes / KB)} KB (limit ${MAX_ENVIRONMENT_FILE_BYTES / KB} KB)`);
    files.push({ path: p, content: f.content });
  }
  if (total > MAX_ENVIRONMENT_TOTAL_BYTES) fileProblems.push(`the draft is ${Math.round(total / KB)} KB (limit ${MAX_ENVIRONMENT_TOTAL_BYTES / KB} KB)`);
  checks.push(result('files', 'Only the environment is touched', fileProblems, `${files.length} file${files.length === 1 ? '' : 's'}, all under ${prefix} or ${configPath}`));

  // ---- parse the OpenTofu --------------------------------------------------------------------------------------------
  const tf: TfFile[] = [];
  const syntax: string[] = [];
  for (const f of files.filter((x) => isTf(x.path))) {
    const parsed = parseHcl(f.content);
    for (const e of parsed.errors) syntax.push(`${f.path}: ${e}`);
    tf.push({ path: f.path, blocks: parsed.blocks, text: f.content });
  }
  checks.push(result('syntax', 'OpenTofu is readable', syntax, tf.length === 0 ? 'no OpenTofu in this draft' : `${tf.length} .tf file${tf.length === 1 ? '' : 's'} read`));

  const all = tf.flatMap((f) => [...walkBlocks(f.blocks)].map((b) => ({ file: f.path, block: b })));
  const where = (file: string, b: HclBlock): string => `${file}:${b.line}`;

  // ---- config ----------------------------------------------------------------------------------------------------------
  const configProblems: string[] = [];
  const configFile = files.find((f) => f.path === configPath);
  let hardware = input.hardware ?? 'c7i.large';
  if (input.moduleOnly) {
    // the module alone is re-checked at apply time: there is no ratchet.yaml to compare
  } else if (!configFile) {
    configProblems.push(`${configPath} must be part of the draft (it records environment.scope)`);
  } else {
    const parsed = parseConfig(configFile.content);
    if (!parsed.ok) {
      for (const e of parsed.errors) configProblems.push(`${configPath}: ${formatConfigError(e)}`);
    } else {
      hardware = parsed.config.evaluator.hardware;
      const declared = parsed.config.environment?.scope;
      if (declared !== input.scope) configProblems.push(`${configPath} declares environment.scope ${declared ?? '(none)'}, the draft is ${input.scope}`);
      if (input.originalConfigYaml !== undefined) {
        try {
          const before = (parseYaml(input.originalConfigYaml) ?? {}) as Record<string, unknown>;
          const after = (parseYaml(configFile.content) ?? {}) as Record<string, unknown>;
          const strip = (o: Record<string, unknown>) => {
            const { environment: _e, ...rest } = o;
            return rest;
          };
          if (stableJson(strip(before)) !== stableJson(strip(after))) {
            const keys = new Set([...Object.keys(strip(before)), ...Object.keys(strip(after))]);
            const changed = [...keys].filter((k) => stableJson(before[k]) !== stableJson(after[k]));
            configProblems.push(`${configPath} changes ${changed.join(', ')}: a draft may only change environment (what is measured, the guardrails and the protected paths are the user's)`);
          }
          const envBefore = (before['environment'] ?? {}) as Record<string, unknown>;
          const envAfter = (after['environment'] ?? {}) as Record<string, unknown>;
          const nets = (e: Record<string, unknown>) => (Array.isArray(e['network']) ? (e['network'] as unknown[]).map(String) : []);
          const added = nets(envAfter).filter((h) => !nets(envBefore).includes(h));
          if (added.length > 0) configProblems.push(`environment.network adds ${added.join(', ')}: egress hosts are the user's to grant, not a draft's`);
        } catch (err) {
          configProblems.push(`cannot compare ${configPath} with the base ref: ${(err as Error).message}`);
        }
      }
    }
  }
  checks.push(result('config', 'Only the environment section of ratchet.yaml changes', configProblems, 'what is measured, the guardrails and the protected paths are untouched'));

  // ---- the module call + machines ------------------------------------------------------------------------------------
  const modProblems: string[] = [];
  const sameProblems: string[] = [];
  const sealedProblems: string[] = [];
  const scopeProblems: string[] = [];
  const modules = all.filter((x) => x.block.type === 'module');
  let machines: MachineSpec[] = [];
  const bag = { same: sameProblems, sealed: sealedProblems, scope: scopeProblems, module: modProblems };

  for (const { file, block } of modules) {
    const name = block.labels[0];
    const source = literalString(block.attrs['source']);
    if (name !== SANDBOX_BASE_MODULE_NAME || source !== SANDBOX_BASE_MODULE_SOURCE) {
      modProblems.push(`${where(file, block)}: module "${name ?? '?'}" (source ${source ? JSON.stringify(source) : 'is not a literal'}) is not allowed: the only module a draft may call is module "${SANDBOX_BASE_MODULE_NAME}" with source "${SANDBOX_BASE_MODULE_SOURCE}"`);
    }
  }
  const baseCalls = modules.filter((x) => x.block.labels[0] === SANDBOX_BASE_MODULE_NAME && literalString(x.block.attrs['source']) === SANDBOX_BASE_MODULE_SOURCE);
  if (tf.length > 0 && baseCalls.length === 0) modProblems.push(`missing the base module: call module "${SANDBOX_BASE_MODULE_NAME}" with source "${SANDBOX_BASE_MODULE_SOURCE}" (it brings the network isolation, the evaluation identity, the sealed results and the budget tags)`);
  if (baseCalls.length > 1) modProblems.push('the base module is called more than once');
  for (const { file, block } of baseCalls) {
    const r = block.attrs['ratchet'];
    if (!r || r.kind !== 'expr' || r.text.replace(/\s/g, '') !== 'var.ratchet') modProblems.push(`${where(file, block)}: pass the sandbox through with \`ratchet = var.ratchet\``);
    for (const k of ['count', 'for_each', 'providers', 'depends_on']) {
      if (k in block.attrs) modProblems.push(`${where(file, block)}: the module call may not set ${k}`);
    }
    for (const k of Object.keys(block.attrs)) {
      if (!['source', 'ratchet', 'machines'].includes(k)) modProblems.push(`${where(file, block)}: unknown module argument "${k}"`);
    }
    machines = readMachines(block, bag);
    const seenLabels = new Set<string>();
    for (const m of machines) {
      if (seenLabels.has(m.name)) modProblems.push(`two machines are called "${m.name}"`);
      seenLabels.add(m.name);
    }
  }
  if (machines.length > MAX_ENVIRONMENT_MACHINES) modProblems.push(`${machines.length} machines (limit ${MAX_ENVIRONMENT_MACHINES})`);
  for (const { file, block } of all.filter((x) => x.block.type === 'variable')) {
    if (block.labels[0] === 'ratchet') modProblems.push(`${where(file, block)}: variable "ratchet" is provided by Ratchet; a draft must not declare it`);
  }
  for (const { file, block } of all.filter((x) => x.block.type === 'output')) {
    if (block.labels[0] === 'environment') modProblems.push(`${where(file, block)}: output "environment" is generated by Ratchet`);
  }
  for (const { file, block } of all.filter((x) => x.block.type === 'terraform')) {
    if (block.blocks.some((b) => b.type === 'backend' || b.type === 'cloud')) modProblems.push(`${where(file, block)}: a backend would move the state out of Ratchet's hands`);
    if (block.blocks.some((b) => b.type === 'required_providers')) modProblems.push(`${where(file, block)}: providers are pinned by Ratchet, a draft declares none`);
  }
  for (const { file, block } of all.filter((x) => x.block.type === 'provider')) {
    sealedProblems.push(`${where(file, block)}: provider "${block.labels[0] ?? '?'}" would let the draft reach another region or account; Ratchet configures the provider`);
  }
  checks.push(result('base-module', 'Built on the base module Ratchet ships', modProblems, tf.length === 0 ? 'no infrastructure is drafted (nothing to build on)' : `module "${SANDBOX_BASE_MODULE_NAME}" with ${machines.length} machine${machines.length === 1 ? '' : 's'}`));

  // ---- resources allow-list, isolation, local access, teardown, tags -----------------------------------------------------
  const isolated: string[] = [];
  const local: string[] = [];
  const teardown: string[] = [];
  const tagged: string[] = [];
  const knownFiles = new Set(files.map((f) => f.path));

  for (const { file, block } of all) {
    const w = where(file, block);
    if (block.type === 'resource') {
      const type = block.labels[0] ?? '';
      if (!ALLOWED_RESOURCE_TYPES.has(type)) {
        const net = /^aws_(internet_gateway|nat_gateway|eip|eip_association|route|route_table|route_table_association|vpc|subnet|vpc_peering|vpn|customer_gateway|ec2_transit|lb|alb|elb|globalaccelerator|cloudfront|api_gateway|apigateway|network_interface|network_acl|security_group|vpc_security_group|vpc_endpoint|instance|launch_template|autoscaling|spot)/.test(type);
        const iam = /^aws_(iam|organizations|sso|kms_key_policy)/.test(type);
        const msg = `${w}: resource ${type} is not allowed`;
        if (net) isolated.push(`${msg}: network and machines come from the base module (measuring hosts have no route to the internet)`);
        else if (iam) sealedProblems.push(`${msg}: identities come from the base module (evaluation only; agents cannot read results or the holdout)`);
        else isolated.push(`${msg} (allowed: ${[...ALLOWED_RESOURCE_TYPES].join(', ')}; machines go in the base module's \`machines\`)`);
      }
      if (type === 'aws_s3_object') {
        const bucket = block.attrs['bucket'];
        const bucketExpr = bucket?.kind === 'expr' ? bucket.text.replace(/\s/g, '') : '';
        if (bucketExpr !== 'module.sandbox.artifacts_bucket' && bucketExpr !== 'var.ratchet.artifacts_bucket') sealedProblems.push(`${w}: aws_s3_object must use bucket = module.sandbox.artifacts_bucket`);
        const src = block.attrs['source'];
        if (src) {
          const text = src.kind === 'string' ? src.value : '';
          if (!/^\$\{path\.module\}\/[^.]/.test(text) || text.includes('..')) local.push(`${w}: source may only read a file of the draft: "\${path.module}/<file>"`);
        }
        const key = literalString(block.attrs['key']);
        const keyDyn = block.attrs['key']?.kind === 'string' ? (block.attrs['key'] as { value: string }).value : '';
        const k = key ?? keyDyn;
        if (!k.startsWith(ENVIRONMENT_OBJECT_PREFIX)) sealedProblems.push(`${w}: aws_s3_object keys must start with "${ENVIRONMENT_OBJECT_PREFIX}" (results and the holdout live elsewhere)`);
      }
      if (type !== 'random_id') {
        const t = block.attrs['tags'];
        if (!t || !/module\.sandbox\.tags/.test(t.kind === 'expr' ? t.text : JSON.stringify(t))) tagged.push(`${w}: ${type}.${block.labels[1] ?? '?'} must set tags = module.sandbox.tags (or merge it in)`);
      }
    } else if (block.type === 'data') {
      const type = block.labels[0] ?? '';
      if (!ALLOWED_DATA_TYPES.has(type)) local.push(`${w}: data source ${type} is not allowed (allowed: ${[...ALLOWED_DATA_TYPES].join(', ')})`);
    } else if (block.type === 'provisioner') {
      local.push(`${w}: provisioner "${block.labels[0] ?? ''}" runs commands; a drafted environment only declares resources`);
    } else if (block.type === 'import' || block.type === 'removed' || block.type === 'moved') {
      teardown.push(`${w}: ${block.type} blocks are not allowed (they adopt or orphan resources, so teardown could not remove everything)`);
    } else if (block.type === 'lifecycle') {
      const pd = block.attrs['prevent_destroy'];
      if (pd?.kind === 'bool' && pd.value) teardown.push(`${w}: prevent_destroy blocks teardown`);
    }
    for (const [k, v] of Object.entries(block.attrs)) {
      if (['deletion_protection', 'skip_destroy', 'prevent_destroy', 'delete_protection', 'disable_api_termination', 'termination_protection'].includes(k) && v.kind === 'bool' && v.value) {
        teardown.push(`${w}: ${k} = true blocks teardown`);
      }
      if (k === 'force_destroy' && v.kind === 'bool' && !v.value) teardown.push(`${w}: force_destroy = false can leave objects behind`);
      if (k === 'associate_public_ip_address' && !(v.kind === 'bool' && !v.value)) isolated.push(`${w}: public addresses are not allowed`);
    }
  }
  for (const f of tf) {
    if (/(?<![\d.])0\.0\.0\.0\/0|::\/0/.test(f.text)) isolated.push(`${f.path}: open CIDR ranges (0.0.0.0/0, ::/0) are not allowed: measuring hosts have no route to the internet`);
    if (/\b(?:local-exec|remote-exec)\b/.test(f.text) || /data\s+"external"/.test(f.text)) local.push(`${f.path}: commands and external data sources are not allowed`);
    if (/\bpath\.(?:cwd|root)\b/.test(f.text)) local.push(`${f.path}: path.cwd / path.root reach outside the draft; use \${path.module}`);
    if (/\b(?:sealed|holdout|bench-results|confirm_identity|eval_identity|agent_boundary)\b/.test(f.text.replace(/^\s*(#|\/\/).*$/gm, ''))) {
      sealedProblems.push(`${f.path}: refers to the sealed store, holdout, results or identities, which a draft never touches`);
    }
    if (/\biam_instance_profile\b|\binstance_profile\b/.test(f.text)) sealedProblems.push(`${f.path}: identities are not set by a draft`);
    for (const c of fileCalls(f.text)) {
      const m = /^"\$\{path\.module\}\/([^"]+)"/.exec(c.arg);
      if (!m || m[1]!.includes('..')) {
        local.push(`${f.path}: ${c.fn}(${c.arg.slice(0, 60)}) must read a file inside the draft: ${c.fn}("\${path.module}/<file>")`);
      } else if (c.fn !== 'fileexists' && c.fn !== 'fileset' && !knownFiles.has(`${prefix}${m[1]}`)) {
        local.push(`${f.path}: ${c.fn}() reads ${m[1]}, which is not part of the draft`);
      }
    }
  }
  for (const f of files.filter((x) => x.path.startsWith(prefix) && !isTf(x.path) && !/\.(md|txt|json|ya?ml|csv|sql)$/i.test(x.path))) {
    const lines = f.content.split('\n').filter((l) => !/^\s*#/.test(l));
    const hit = INTERNET_IN_SCRIPT.find((re) => lines.some((l) => re.test(l)));
    if (hit) {
      const line = lines.find((l) => hit.test(l))!.trim().slice(0, 80);
      isolated.push(`${f.path}: \`${line}\` needs the internet; measuring hosts have no route out (ship what they need in the repo's container or the seed data)`);
    }
  }
  const isoOk = tf.length === 0 ? 'no infrastructure is drafted' : 'measuring hosts sit in the sandbox\'s private subnet: no public address, no route to the internet';
  checks.push(result('isolated', 'Isolated: no internet on the measuring hosts', isolated, isoOk));
  checks.push(result('local-access', 'Reads and runs nothing on your machine', local, 'only files of the draft are read; no commands run'));
  checks.push(result('sealed', "Results sealed: agents can't read results or the holdout", sealedProblems, 'the evaluation identity is the base module\'s; objects live under env/'));
  checks.push(result('tagged', 'Everything tagged for the budget', tagged, 'the base module tags every machine and volume; nothing else is created untagged'));
  checks.push(result('teardown', 'Full teardown', teardown, 'nothing blocks, adopts or orphans a resource: destroy removes everything'));

  // ---- same hardware -----------------------------------------------------------------------------------------------------
  for (const m of machines) {
    if ((m.role === 'app' || m.role === 'evaluator' || m.role === 'load') && /^t\d/.test(m.machine)) {
      sameProblems.push(`"${m.name}" is ${m.machine}: burstable instances change speed with CPU credits; pin a fixed-performance type (c7i, m7i, r7i) for ${m.role}`);
    }
  }
  checks.push(result('same-hardware', 'Same hardware every time', sameProblems, machines.length === 0 ? "the evaluator's pinned machine is used" : `${machines.length} machine${machines.length === 1 ? '' : 's'}, each a literal instance type`));

  // ---- scope ---------------------------------------------------------------------------------------------------------------
  const infraFiles = files.filter((f) => f.path.startsWith(prefix));
  const hasTf = tf.length > 0;
  if (input.scope === 'none' && infraFiles.length > 0) scopeProblems.push(`scope none needs no infrastructure, but ${infraFiles.length} file(s) under ${prefix} were drafted`);
  if (input.scope === 'single' && hasTf && machines.length !== 1) scopeProblems.push(`scope single is one machine, the draft has ${machines.length}; use scope full for a stack`);
  if (input.scope === 'full') {
    if (!hasTf) scopeProblems.push(`scope full needs OpenTofu (${prefix}main.tf)`);
    else if (machines.length < 2) scopeProblems.push('scope full is a stack of at least two machines (for one machine use scope single)');
  }
  checks.push(result('scope', 'The scope fits the files', scopeProblems, `scope ${input.scope}`));

  // ---- cost ----------------------------------------------------------------------------------------------------------------
  const specs = hasTf || input.scope === 'full' ? machines : defaultMachines(input.scope, hardware);
  const est = estimateMachinesCost(specs, { timeLimitSeconds, ...(input.prices ? { prices: input.prices } : {}) });
  const costProblems = est.unpriced.map((t) => `no price known for ${t}: use a type from ${Object.keys(INSTANCE_USD_PER_HOUR).slice(0, 4).join(', ')}, ...`);
  checks.push(result('cost', 'Priced before it is created', costProblems, `$${est.cost.usdPerHour.toFixed(2)}/h, ready in ~${est.cost.startupMinutes} min`));

  const order = new Map<string, number>(ENVIRONMENT_CHECK_IDS.map((id, i) => [id, i]));
  checks.sort((a, b) => (order.get(a.id) ?? 99) - (order.get(b.id) ?? 99));
  return {
    ok: checks.every((c) => c.ok),
    checks,
    machines: specs,
    resources: est.resources,
    cost: est.cost,
    problems: checks.filter((c) => !c.ok).map((c) => `${c.label}: ${c.detail}`),
  };
}
