import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { chmodSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type {
  AgentIdentity,
  BenchLaunchSpec,
  CloudCompute,
  InstanceInfo,
  RemoteCommand,
  RemoteCommandResult,
  RunnerLaunchSpec,
  SecretRef,
} from '@ratchet/core';

/**
 * CloudCompute fake whose "bench instance" is a local bash: `runCommand` writes the generated
 * script to a file and executes it, so the real scripts are exercised. Like SSM, stdout is
 * truncated to 24,000 characters.
 *
 * `s3Dir` maps `s3://<bucket>/<key>` to `<s3Dir>/<bucket>/<key>` through an `aws` shim on PATH.
 */
export class LocalFakeCompute implements CloudCompute {
  launched: BenchLaunchSpec[] = [];
  terminated: string[] = [];
  commands: Array<{ id: string; script: string }> = [];
  failTerminate = false;
  /** Make `runCommand` throw this many times before working (instance not yet registered with SSM). */
  notReadyFor = 0;
  private children = new Set<ChildProcess>();
  private counter = 0;

  constructor(
    private readonly scratch: string,
    private readonly extraEnv: Record<string, string> = {},
    s3Dir?: string,
  ) {
    mkdirSync(scratch, { recursive: true });
    this.shim = join(scratch, 'shim');
    mkdirSync(this.shim, { recursive: true });
    if (s3Dir) {
      const aws = join(this.shim, 'aws');
      writeFileSync(
        aws,
        `#!/bin/bash\n# aws s3 cp [--only-show-errors] s3://b/k dst\nargs=(); for a in "$@"; do case "$a" in --*) ;; *) args+=("$a");; esac; done\nsrc="\${args[2]}"; dst="\${args[3]}"\ncp -- ${JSON.stringify(s3Dir)}/"\${src#s3://}" "$dst"\n`,
      );
      chmodSync(aws, 0o755);
    }
  }
  private readonly shim: string;

  /** Install an executable shim ahead of the real binary on the fake instance's PATH. */
  addShim(name: string, body: string): void {
    const p = join(this.shim, name);
    writeFileSync(p, `#!/bin/bash\n${body}\n`);
    chmodSync(p, 0o755);
  }

  async launchBenchInstance(spec: BenchLaunchSpec): Promise<InstanceInfo> {
    this.launched.push(spec);
    return {
      id: `i-fake${++this.counter}`,
      role: 'bench',
      optimizationId: spec.optimizationId,
      instanceType: spec.instanceType,
      state: 'running',
      launchedAt: Date.now(),
    };
  }

  async terminate(ids: string[]): Promise<void> {
    if (this.failTerminate) throw new Error('terminate failed');
    this.terminated.push(...ids);
    for (const c of this.children) if (c.pid) this.kill(c);
  }

  /** Terminating an instance kills everything on it, including processes that left the process group (`timeout` does). */
  private kill(c: ChildProcess): void {
    const tree = (pid: number): number[] => {
      let kids: number[] = [];
      try {
        kids = execFileSync('pgrep', ['-P', String(pid)], { encoding: 'utf8' }).split('\n').filter(Boolean).map(Number);
      } catch {
        // no children
      }
      return [pid, ...kids.flatMap(tree)];
    };
    for (const pid of tree(c.pid!).reverse()) {
      try {
        process.kill(pid, 'SIGKILL');
      } catch {
        // gone
      }
    }
  }

  async runCommand(instanceId: string, command: RemoteCommand): Promise<RemoteCommandResult> {
    if (this.notReadyFor > 0) {
      this.notReadyFor--;
      throw new Error('InvalidInstanceId: instance not registered');
    }
    this.commands.push({ id: instanceId, script: command.script });
    const file = join(this.scratch, `script-${this.commands.length}.sh`);
    writeFileSync(file, command.script);
    return new Promise((resolve) => {
      const child = spawn('bash', [file], {
        env: { ...process.env, PATH: `${this.shim}:${process.env.PATH}`, ...this.extraEnv, ...command.env },
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: true,
      });
      this.children.add(child);
      let stdout = '';
      let stderr = '';
      child.stdout.setEncoding('utf8').on('data', (d: string) => (stdout += d));
      child.stderr.setEncoding('utf8').on('data', (d: string) => (stderr += d));
      child.on('close', (code) => {
        this.children.delete(child);
        resolve({ exitCode: code ?? 137, stdout: stdout.slice(0, 24000), stderr: stderr.slice(0, 24000) });
      });
    });
  }

  // ---- unused by the bench runner
  async createAgentIdentity(): Promise<AgentIdentity> {
    throw new Error('not implemented');
  }
  async revokeAgentIdentity(): Promise<void> {}
  async deleteAgentIdentity(): Promise<void> {}
  async putSecret(): Promise<SecretRef> {
    throw new Error('not implemented');
  }
  async deleteSecrets(): Promise<void> {}
  async launchRunner(_spec: RunnerLaunchSpec): Promise<InstanceInfo> {
    throw new Error('not implemented');
  }
  async listTagged(): Promise<InstanceInfo[]> {
    return [];
  }
}
