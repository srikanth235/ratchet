import { lstat, mkdir, readdir, readFile, realpath } from 'node:fs/promises';
import { join, posix, sep } from 'node:path';
import type { GitHost } from '@ratchet/core';

/**
 * Advanced: `infra: <path>` in ratchet.yaml points at a custom OpenTofu module instead of the one that ships with the app
 * (infra/aws). OFF BY DEFAULT. The module is read from the PROTECTED ref only (agents edit the repo, and a module runs with the
 * user's cloud credentials), refused when it can run local commands, and its outputs are validated against the sandbox contract
 * (infra/contract.md) after every apply, so a module that does not satisfy the contract fails loudly instead of weakening the
 * isolation silently.
 */

export class InfraOverrideError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'InfraOverrideError';
  }
}

/** `config.infra` (defensive: the schema key is added elsewhere). Undefined when absent; a repo-relative path otherwise. */
export function readInfraOverride(config: unknown): string | undefined {
  // TODO(schema): use the typed `config.infra` once packages/core declares it.
  const v = (config as { infra?: unknown } | null | undefined)?.infra;
  if (v === undefined || v === null || v === '') return undefined;
  if (typeof v !== 'string') throw new InfraOverrideError('infra must be a path to an OpenTofu module inside the repository');
  if (v.includes('\0') || v.startsWith('/') || /^[A-Za-z]:/.test(v)) throw new InfraOverrideError(`infra must be a path inside the repository, not ${JSON.stringify(v)}`);
  const n = posix.normalize(v.replace(/\\/g, '/')).replace(/\/+$/, '');
  if (n === '..' || n.startsWith('../')) throw new InfraOverrideError(`infra must stay inside the repository (${JSON.stringify(v)})`);
  if (n === '.' || n === '') throw new InfraOverrideError('infra must name a directory of the repository, not the repository root');
  return n;
}

const LOCAL_EXEC = /provisioner\s+"(local-exec|remote-exec)"|data\s+"external"|\bexternal\s*=\s*\{|program\s*=\s*\[/;

/**
 * Checks `ref` out (the protected ref, resolved to a commit) and returns the module directory. Refuses a module that is not a
 * real directory inside the checkout (symlinks), has no .tf files, or can run commands on this machine during plan / apply.
 */
export async function materializeInfraOverride(a: {
  git: Pick<GitHost, 'checkout' | 'resolveRef'>;
  /** The protected ref (`bench.ref`). Never a candidate. */
  ref: string;
  /** From `readInfraOverride`. */
  path: string;
  /** Empty parent directory the checkout goes under. */
  destRoot: string;
}): Promise<{ moduleDir: string; commit: string }> {
  const commit = await a.git.resolveRef(a.ref);
  await mkdir(a.destRoot, { recursive: true });
  const root = join(a.destRoot, `infra-${commit.slice(0, 12)}`);
  await a.git.checkout(commit, root);
  const moduleDir = join(root, ...a.path.split('/'));
  // No symlink anywhere on the way: the module must be what the protected ref contains.
  let acc = root;
  for (const part of a.path.split('/')) {
    acc = join(acc, part);
    const st = await lstat(acc).catch(() => undefined);
    if (!st) throw new InfraOverrideError(`infra ${a.path} does not exist at ${a.ref}`);
    if (st.isSymbolicLink() || !st.isDirectory()) throw new InfraOverrideError(`infra ${a.path} must be a plain directory at ${a.ref} (no symlinks)`);
  }
  const real = await realpath(moduleDir);
  if (!(real + sep).startsWith((await realpath(root)) + sep)) throw new InfraOverrideError(`infra ${a.path} resolves outside the repository`);
  const tf = (await readdir(moduleDir)).filter((f) => f.endsWith('.tf'));
  if (tf.length === 0) throw new InfraOverrideError(`infra ${a.path} at ${a.ref} contains no .tf files`);
  for (const f of tf) {
    const text = await readFile(join(moduleDir, f), 'utf8');
    if (LOCAL_EXEC.test(text)) {
      throw new InfraOverrideError(`infra ${a.path}/${f} can run commands on this machine (local-exec, remote-exec or an external data source); a custom sandbox module may only declare cloud resources`);
    }
  }
  return { moduleDir, commit };
}

// ---- the sandbox contract, checked on the outputs ------------------------------------------------------------------------

export interface ContractCheck {
  ok: boolean;
  errors: string[];
}

export interface ContractCheckOptions {
  /**
   * `deps_identity_ref` is optional in the contract (sandboxes from before dependency snapshots), but without it the app can
   * neither build dependency snapshots nor container images. Default true: a custom module must provide it.
   */
  requireDeps?: boolean;
}

export class ContractViolationError extends Error {
  constructor(readonly errors: string[]) {
    super(
      `the custom sandbox module does not satisfy the sandbox contract (infra/contract.md):\n - ${errors.join('\n - ')}\n` +
        'Fix the module so its outputs match the contract, or remove `infra:` from ratchet.yaml to use the module that ships with the app.',
    );
    this.name = 'ContractViolationError';
  }
}

const BUCKET = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/;
const ARN = (service: string, kind: string) => new RegExp(`^arn:aws[a-z-]*:${service}::?\\d{12}:${kind}/.+`);
const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);

/**
 * Validates `tofu output -json` (name -> {value}) against infra/contract.md: every output the app reads is present and has the
 * shape it expects, and the separations the invariants depend on hold (the three bench identities differ, the sealed store is a
 * different bucket from the artifacts store, bench machines are not in the agent subnet or group).
 */
export function checkSandboxContract(doc: unknown, opts: ContractCheckOptions = {}): ContractCheck {
  const errors: string[] = [];
  if (!isObj(doc)) return { ok: false, errors: ['the module produced no outputs (tofu output -json is not an object)'] };
  const val = (name: string): unknown => (isObj(doc[name]) ? (doc[name] as { value?: unknown }).value : undefined);
  const has = (name: string): boolean => isObj(doc[name]) && 'value' in (doc[name] as object);
  const need = (name: string): unknown => {
    if (!has(name)) errors.push(`output "${name}" is missing`);
    return val(name);
  };
  const str = (name: string, v: unknown, re?: RegExp, what = 'a string'): string | undefined => {
    if (typeof v !== 'string' || v === '') {
      // a missing top-level output was already reported by need(); a missing field inside one is reported here
      if (v !== undefined || name.includes('.')) errors.push(`output "${name}" must be ${what}`);
      return undefined;
    }
    if (re && !re.test(v)) {
      errors.push(`output "${name}" must be ${what} (got ${JSON.stringify(v)})`);
      return undefined;
    }
    return v;
  };
  const identity = (name: string, v: unknown): { role: string; profile: string } | undefined => {
    if (v === undefined) return undefined;
    if (!isObj(v)) {
      errors.push(`output "${name}" must be an object with role_arn, role_name, instance_profile_arn and instance_profile_name`);
      return undefined;
    }
    const role = str(`${name}.role_arn`, v.role_arn, ARN('iam', 'role'), 'an IAM role ARN');
    str(`${name}.role_name`, v.role_name);
    const profile = str(`${name}.instance_profile_arn`, v.instance_profile_arn, ARN('iam', 'instance-profile'), 'an IAM instance profile ARN');
    str(`${name}.instance_profile_name`, v.instance_profile_name);
    return role && profile ? { role, profile } : undefined;
  };

  const artifacts = str('artifacts_bucket', need('artifacts_bucket'), BUCKET, 'an S3 bucket name');
  const sealed = str('sealed_bucket', need('sealed_bucket'), BUCKET, 'an S3 bucket name');
  if (artifacts && sealed && artifacts === sealed) errors.push('artifacts_bucket and sealed_bucket must be different buckets (sealed isolation)');
  str('agent_boundary_ref', need('agent_boundary_ref'), ARN('iam', 'policy'), 'an IAM policy ARN');
  const rolePath = str('agent_role_path', need('agent_role_path'));
  if (rolePath && !(rolePath.startsWith('/') && rolePath.endsWith('/'))) errors.push(`output "agent_role_path" must start and end with "/" (got ${JSON.stringify(rolePath)})`);
  const prefix = str('sealed_confirm_prefix', need('sealed_confirm_prefix'));
  if (prefix && (prefix.startsWith('/') || !prefix.endsWith('/'))) errors.push('output "sealed_confirm_prefix" must be a key prefix ending in "/" (the only part of the sealed store the confirm identity reads)');

  const evalId = identity('eval_identity_ref', need('eval_identity_ref'));
  const confirmId = identity('confirm_identity_ref', need('confirm_identity_ref'));
  const depsRaw = has('deps_identity_ref') ? val('deps_identity_ref') : (opts.requireDeps ?? true) ? need('deps_identity_ref') : undefined;
  const depsId = identity('deps_identity_ref', depsRaw);
  const ids = [evalId, confirmId, depsId].filter((x): x is { role: string; profile: string } => !!x);
  if (new Set(ids.map((i) => i.role)).size !== ids.length || new Set(ids.map((i) => i.profile)).size !== ids.length) {
    errors.push('eval_identity_ref, confirm_identity_ref and deps_identity_ref must be three different identities (least identity per bench phase)');
  }

  const net = need('network_ref');
  if (net !== undefined) {
    if (!isObj(net)) errors.push('output "network_ref" must be an object');
    else {
      str('network_ref.vpc_id', net.vpc_id, /^vpc-[0-9a-f]+$/, 'a VPC id');
      const sub = str('network_ref.subnet_id', net.subnet_id, /^subnet-[0-9a-f]+$/, 'a subnet id');
      const bench = str('network_ref.bench_subnet_id', net.bench_subnet_id, /^subnet-[0-9a-f]+$/, 'a subnet id');
      if (sub && bench && sub === bench) errors.push('network_ref.bench_subnet_id must differ from subnet_id: bench machines must not share the agent subnet (they have no route out)');
      const sg = net.security_group_ids;
      if (!isObj(sg)) errors.push('output "network_ref.security_group_ids" must be an object with agent_runner and bench');
      else {
        const a = str('network_ref.security_group_ids.agent_runner', sg.agent_runner, /^sg-[0-9a-f]+$/, 'a security group id');
        const b = str('network_ref.security_group_ids.bench', sg.bench, /^sg-[0-9a-f]+$/, 'a security group id');
        if (a && b && a === b) errors.push('network_ref.security_group_ids.agent_runner and .bench must be different groups');
      }
      if (net.egress_proxy_url !== null && net.egress_proxy_url !== undefined && typeof net.egress_proxy_url !== 'string') errors.push('output "network_ref.egress_proxy_url" must be a string or null');
      if (!('egress_proxy_url' in net)) errors.push('output "network_ref.egress_proxy_url" is missing (use null without a proxy)');
    }
  }

  const limits = need('limits');
  if (limits !== undefined) {
    if (!isObj(limits)) errors.push('output "limits" must be an object with max_instances_per_agent and max_instance_lifetime_hours');
    else {
      for (const k of ['max_instances_per_agent', 'max_instance_lifetime_hours']) {
        const v = limits[k];
        if (typeof v !== 'number' || !Number.isFinite(v) || v <= 0) errors.push(`output "limits.${k}" must be a positive number`);
      }
    }
  }
  return { ok: errors.length === 0, errors };
}

/** Throws a `ContractViolationError` listing every problem when the outputs (`tofu output -json`) do not satisfy the contract. */
export function assertSandboxContract(json: string | unknown, opts: ContractCheckOptions = {}): void {
  let doc: unknown = json;
  if (typeof json === 'string') {
    try {
      doc = JSON.parse(json);
    } catch {
      throw new ContractViolationError(['the module outputs are not JSON']);
    }
  }
  const r = checkSandboxContract(doc, opts);
  if (!r.ok) throw new ContractViolationError(r.errors);
}
