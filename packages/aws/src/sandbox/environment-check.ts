import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import type { EnvironmentChecker, SandboxEnvironment } from '@ratchet/core';
import { hashEnvironmentFiles } from '@ratchet/core';
import { EnvironmentRejectedError, materializeDraftedEnvironment } from './environment.js';
import { NodeProcessRunner, type ProcessRunner } from './process.js';

export interface TofuEnvironmentCheckerOptions {
  tofuBinary: string;
  /** `infra/aws`: the base module directory is `../modules/sandbox` next to it unless `baseModuleDir` says otherwise. */
  moduleDir: string;
  baseModuleDir?: string;
  /** Where scratch directories go (default: the OS temp dir). */
  workRoot?: string;
  runner?: ProcessRunner;
  /** Extra environment for tofu (merged under AWS_PROFILE). */
  env?: Record<string, string>;
  /** The bench image the plan looks up by tag (the app version), or an explicit AMI id. */
  imageVersion?: string;
  machineImageId?: string;
}

const COMMON = ['-input=false', '-no-color'];

/** Stand-ins for the sandbox's outputs: `tofu plan` needs ids of the right shape, not real ones (nothing is created or refreshed). */
const PLACEHOLDER_SANDBOX = {
  sandbox_id: 'plan-check',
  region: '',
  vpc_id: 'vpc-0plancheck000000',
  bench_subnet_id: 'subnet-0plancheck00000',
  bench_security_group_id: 'sg-0plancheck000000',
  artifacts_bucket: 'ratchet-plan-check-artifacts',
  eval_instance_profile_name: 'ratchet-plan-check-bench-runner',
};

const tail = (s: string, n = 600): string => s.trim().split('\n').slice(-12).join(' ').slice(-n);

/**
 * "Plans cleanly in your AWS account": materializes the draft exactly as the sandbox provider would, then runs `tofu init`,
 * `tofu validate` and `tofu plan` (no refresh, no lock, nothing created) with the user's AWS profile. The plan reads the bench
 * image by tag in the target region, so a missing image or bad credentials surface here, before the user approves, instead of at run start.
 */
export class TofuEnvironmentChecker implements EnvironmentChecker {
  private readonly runner: ProcessRunner;

  constructor(private readonly o: TofuEnvironmentCheckerOptions) {
    this.runner = o.runner ?? new NodeProcessRunner();
  }

  async plan(input: { scope: SandboxEnvironment['scope']; files: Array<{ path: string; content: string }>; region: string; awsProfile?: string | undefined }): Promise<{ ok: boolean; detail: string }> {
    const root = await mkdtemp(join(this.o.workRoot ?? tmpdir(), 'ratchet-plan-check-'));
    try {
      const rel = input.files.map((f) => ({ path: f.path.replace(/^infra\/ratchet\//, ''), content: f.content }));
      const env: SandboxEnvironment = { kind: 'drafted', scope: input.scope, path: 'infra/ratchet', commit: 'draft', hash: hashEnvironmentFiles(rel), files: rel };
      const dest = join(root, 'module');
      try {
        await materializeDraftedEnvironment({ env, baseModuleDir: this.o.baseModuleDir ?? join(dirname(this.o.moduleDir), 'modules', 'sandbox'), destDir: dest });
      } catch (err) {
        if (err instanceof EnvironmentRejectedError) return { ok: false, detail: err.problems.join('; ') };
        throw err;
      }
      const vars = {
        ratchet: { ...PLACEHOLDER_SANDBOX, region: input.region, environment_hash: env.hash, machine_image_id: this.o.machineImageId ?? '', image_version: this.o.imageVersion ?? '', tags: {} },
      };
      const varsPath = join(root, 'vars.tfvars.json');
      await mkdir(root, { recursive: true });
      await writeFile(varsPath, JSON.stringify(vars));
      const tfEnv = {
        ...this.o.env,
        ...(input.awsProfile ? { AWS_PROFILE: input.awsProfile } : {}),
        AWS_REGION: input.region,
        TF_DATA_DIR: join(root, '.terraform-data'),
        TF_IN_AUTOMATION: '1',
        TF_INPUT: '0',
        CHECKPOINT_DISABLE: '1',
      };
      const run = (args: string[], okCodes = [0]) => this.runner.run({ command: this.o.tofuBinary, args, cwd: dest, env: tfEnv }).then((r) => ({ ...r, ok: okCodes.includes(r.exitCode) }));
      const init = await run(['init', ...COMMON, '-backend=false']);
      if (!init.ok) return { ok: false, detail: `tofu init failed: ${tail(init.stderr || init.stdout)}` };
      const validate = await run(['validate', '-no-color']);
      if (!validate.ok) return { ok: false, detail: `tofu validate failed: ${tail(validate.stderr || validate.stdout)}` };
      const plan = await run(['plan', ...COMMON, '-refresh=false', '-lock=false', '-detailed-exitcode', `-var-file=${varsPath}`], [0, 2]);
      if (!plan.ok) return { ok: false, detail: `tofu plan failed in ${input.awsProfile ? `profile ${input.awsProfile}` : 'your AWS account'}: ${tail(plan.stderr || plan.stdout)}` };
      const adds = /Plan: (\d+) to add/.exec(plan.stdout)?.[1];
      return { ok: true, detail: `tofu validate and plan succeeded${adds ? ` (${adds} resources to add)` : ''} in ${input.region}` };
    } finally {
      await rm(root, { recursive: true, force: true }).catch(() => undefined);
    }
  }
}
