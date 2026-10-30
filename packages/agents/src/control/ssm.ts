import type { AgentEvent, ArtifactStore, CloudCompute, RunnerConfig, RunnerControl, RunnerHandle, RunnerMessage } from '@ratchet/core';
import { configKey, inboxKey, lastSeq, readEvents, runnerPrefix } from '../artifacts.js';

export interface SsmRunnerControlOptions {
  compute: CloudCompute;
  /** The artifacts bucket (keys are `artifacts/<opt>/<agent>/...`). */
  store: ArtifactStore;
  /** Executable the AMI installs. Default `ratchet-runner`. */
  runnerCommand?: string;
  /** Extra environment for the runner process (RATCHET_ARTIFACTS_BUCKET, RATCHET_SECRET_REFS, AWS_REGION, ...). */
  runnerEnv?: Record<string, string>;
  /** Directory on the instance for pid and log files. Default /var/lib/ratchet. */
  stateDir?: string;
}

const q = (s: string): string => `'${s.replace(/'/g, `'\\''`)}'`;

/**
 * Live-mode `RunnerControl`: the engine never talks to the runner directly.
 *  - start: config as `config.json` in the artifacts store, then an SSM command launches `ratchet-runner` detached
 *  - send:  numbered objects `inbox/00000001.json`
 *  - poll:  numbered JSONL batches `events/00000001.jsonl`, cursor = highest batch read
 *  - stop:  a `stop` message plus SIGTERM to the runner pid (the runner aborts its agent's process group)
 */
export class SsmRunnerControl implements RunnerControl {
  private readonly nextInbox = new Map<string, number>();
  private readonly runnerCommand: string;
  private readonly stateDir: string;

  constructor(private readonly options: SsmRunnerControlOptions) {
    this.runnerCommand = options.runnerCommand ?? 'ratchet-runner';
    this.stateDir = options.stateDir ?? '/var/lib/ratchet';
  }

  private key = (h: RunnerHandle) => `${h.optimizationId}/${h.agentId}`;
  private prefix = (h: RunnerHandle) => runnerPrefix(h.optimizationId, h.agentId);
  private pidFile = (h: RunnerHandle) => `${this.stateDir}/${h.optimizationId}-${h.agentId}.pid`;

  async start(handle: RunnerHandle, config: RunnerConfig): Promise<void> {
    const prefix = this.prefix(handle);
    await this.options.store.put(configKey(prefix), JSON.stringify(config));
    const env = {
      RATCHET_OPTIMIZATION_ID: handle.optimizationId,
      RATCHET_AGENT_ID: handle.agentId,
      ...this.options.runnerEnv,
    };
    const assignments = Object.entries(env).map(([k, v]) => `${k}=${q(v)}`).join(' ');
    const log = `${this.stateDir}/${handle.optimizationId}-${handle.agentId}.log`;
    const script = [
      'set -eu',
      `mkdir -p ${q(this.stateDir)}`,
      `if [ -f ${q(this.pidFile(handle))} ] && kill -0 "$(cat ${q(this.pidFile(handle))})" 2>/dev/null; then echo "runner already running" >&2; exit 1; fi`,
      `setsid nohup env ${assignments} ${this.runnerCommand} > ${q(log)} 2>&1 < /dev/null &`,
      `echo $! > ${q(this.pidFile(handle))}`,
    ].join('\n');
    const res = await this.options.compute.runCommand(handle.instanceId, { script, timeoutSeconds: 60 });
    if (res.exitCode !== 0) throw new Error(`failed to start runner on ${handle.instanceId}: ${res.stderr.trim() || res.stdout.trim()}`);
  }

  async send(handle: RunnerHandle, message: RunnerMessage): Promise<void> {
    const prefix = this.prefix(handle);
    const k = this.key(handle);
    let n = this.nextInbox.get(k);
    if (n === undefined) n = (await lastSeq(this.options.store, `${prefix}inbox/`)) + 1;
    this.nextInbox.set(k, n + 1);
    await this.options.store.put(inboxKey(prefix, n), JSON.stringify(message));
  }

  poll(handle: RunnerHandle, cursor?: string): Promise<{ events: AgentEvent[]; cursor: string }> {
    return readEvents(this.options.store, this.prefix(handle), cursor);
  }

  async stop(handle: RunnerHandle): Promise<void> {
    await this.send(handle, { type: 'stop' });
    const script = `if [ -f ${q(this.pidFile(handle))} ]; then kill -TERM "$(cat ${q(this.pidFile(handle))})" 2>/dev/null || true; fi`;
    // best effort: the instance may already be gone (kill switch), and the stop message is durable
    await this.options.compute.runCommand(handle.instanceId, { script, timeoutSeconds: 30 }).catch(() => undefined);
  }
}
