import { spawn } from 'node:child_process';

export interface ProcessSpec {
  command: string;
  args: string[];
  cwd: string;
  env: Record<string, string>;
}

export interface ProcessResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

/** Injectable process runner so tests (and simulation) never spawn `tofu`. */
export interface ProcessRunner {
  run(spec: ProcessSpec, signal?: AbortSignal): Promise<ProcessResult>;
}

export class NodeProcessRunner implements ProcessRunner {
  run(spec: ProcessSpec, signal?: AbortSignal): Promise<ProcessResult> {
    return new Promise((resolve, reject) => {
      const child = spawn(spec.command, spec.args, {
        cwd: spec.cwd,
        env: { ...process.env, ...spec.env },
        stdio: ['ignore', 'pipe', 'pipe'],
        signal,
      });
      let stdout = '';
      let stderr = '';
      child.stdout.on('data', (d) => (stdout += d));
      child.stderr.on('data', (d) => (stderr += d));
      child.on('error', reject);
      child.on('close', (code) => resolve({ exitCode: code ?? -1, stdout, stderr }));
    });
  }
}
