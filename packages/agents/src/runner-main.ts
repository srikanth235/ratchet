import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readFile } from 'node:fs/promises';
import type { AgentAdapter, ArtifactStore, Clock, RunnerConfig } from '@ratchet/core';
import {
  ArtifactEventSink,
  ArtifactInbox,
  AwsCliArtifactStore,
  FsArtifactStore,
  configKey,
  doneKey,
  runnerPrefix,
} from './artifacts.js';
import { AcceleratedClock, systemClock } from './clock.js';
import { ClaudeCodeAdapter, type ClaudePermissionMode } from './claude-code.js';
import { CodexAdapter } from './codex.js';
import { loadDemoSearchScript, loadScript } from './demo-scripts.js';
import { execGit, type GitRunner } from './git.js';
import { nodeSpawner, runProcess, type Spawner } from './process.js';
import { runRunnerLoop, type RunnerLoopResult } from './runner-loop.js';
import { ScriptedAgent } from './scripted.js';
import { gitAuthEnv, prepareWorkdir } from './workdir.js';

export interface RunnerMainOptions {
  env: Record<string, string | undefined>;
  spawner?: Spawner;
  git?: GitRunner;
  /** Replaces the S3/filesystem store chosen from the environment (tests). */
  store?: ArtifactStore;
  clock?: Clock;
  signal?: AbortSignal;
  /** Replaces the adapter chosen from `config.kind` (tests). */
  adapterFactory?: (config: RunnerConfig, agentEnv: Record<string, string>) => AgentAdapter;
  log?: (line: string) => void;
}

/** Env passed through to the agent CLI; deliberately small. The git token never goes in. */
const AGENT_ENV_PASSTHROUGH = ['PATH', 'HOME', 'USER', 'LANG', 'LC_ALL', 'TERM', 'TMPDIR', 'SHELL', 'AWS_REGION', 'AWS_DEFAULT_REGION'];

/** `NAME=ref,NAME2=ref2` of SSM parameter names, fetched with `aws ssm get-parameter --with-decryption`. */
export function parseSecretRefs(spec: string | undefined): Record<string, string> {
  const refs: Record<string, string> = {};
  for (const part of (spec ?? '').split(',').map((s) => s.trim()).filter(Boolean)) {
    const eq = part.indexOf('=');
    if (eq <= 0) throw new Error(`bad RATCHET_SECRET_REFS entry '${part}' (want NAME=/ssm/parameter/name)`);
    refs[part.slice(0, eq)] = part.slice(eq + 1);
  }
  return refs;
}

async function fetchSecret(spawner: Spawner, env: Record<string, string>, name: string, ref: string): Promise<string> {
  let out = '';
  const res = await runProcess(spawner, 'aws', ['ssm', 'get-parameter', '--with-decryption', '--name', ref, '--query', 'Parameter.Value', '--output', 'text'], {
    cwd: process.cwd(),
    env,
    onStdoutLine: (l) => (out += l),
  });
  if (res.code !== 0) throw new Error(`cannot read secret ${name} from SSM ${ref}: ${res.stderrTail}`);
  return out;
}

function required(env: Record<string, string | undefined>, name: string): string {
  const v = env[name];
  if (!v) throw new Error(`${name} is required`);
  return v;
}

export function validateConfig(value: unknown): RunnerConfig {
  const c = value as Partial<RunnerConfig> | null;
  const missing = ['optimizationId', 'agentId', 'kind', 'repoUrl', 'branch', 'baseRef', 'config', 'brief', 'baselineValue', 'budgetUsd', 'deadlineAt'].filter(
    (k) => !c || (c as Record<string, unknown>)[k] === undefined,
  );
  if (missing.length) throw new Error(`runner config is missing: ${missing.join(', ')}`);
  return c as RunnerConfig;
}

export function defaultAdapter(config: RunnerConfig, env: Record<string, string | undefined>, agentEnv: Record<string, string>, spawner: Spawner, clock: Clock): AgentAdapter {
  switch (config.kind) {
    case 'claude-code':
      return new ClaudeCodeAdapter({
        env: agentEnv,
        spawner,
        clock,
        ...(env['RATCHET_CLAUDE_BIN'] ? { binary: env['RATCHET_CLAUDE_BIN'] } : {}),
        ...(env['RATCHET_CLAUDE_PERMISSION_MODE'] ? { permissionMode: env['RATCHET_CLAUDE_PERMISSION_MODE'] as ClaudePermissionMode } : {}),
      });
    case 'codex':
      return new CodexAdapter({ env: agentEnv, spawner, clock, ...(env['RATCHET_CODEX_BIN'] ? { binary: env['RATCHET_CODEX_BIN'] } : {}) });
    case 'mock': {
      const which = env['RATCHET_MOCK_SCRIPT'] ?? 'cc';
      const script = which === 'cc' || which === 'cx' ? loadDemoSearchScript(which) : loadScript(which);
      return new ScriptedAgent(script, { clock });
    }
  }
}

/** The body of the `ratchet-runner` binary. Returns the process exit code. */
export async function runnerMain(options: RunnerMainOptions): Promise<number> {
  const { env } = options;
  const log = options.log ?? ((l: string) => process.stderr.write(`${l}\n`));
  const spawner = options.spawner ?? nodeSpawner;
  const speed = Number(env['RATCHET_CLOCK_SPEED'] ?? 1);
  const clock = options.clock ?? (speed !== 1 ? new AcceleratedClock(speed) : systemClock);

  const optimizationId = required(env, 'RATCHET_OPTIMIZATION_ID');
  const agentId = required(env, 'RATCHET_AGENT_ID');
  const prefix = runnerPrefix(optimizationId, agentId);
  const baseEnv = Object.fromEntries(Object.entries(env).filter((e): e is [string, string] => e[1] !== undefined));

  const store: ArtifactStore =
    options.store ??
    (env['RATCHET_ARTIFACTS_BUCKET']
      ? new AwsCliArtifactStore(env['RATCHET_ARTIFACTS_BUCKET'], spawner, baseEnv, env['AWS_REGION'])
      : env['RATCHET_ARTIFACT_DIR']
        ? new FsArtifactStore(env['RATCHET_ARTIFACT_DIR'])
        : (() => {
            throw new Error('set RATCHET_ARTIFACTS_BUCKET (S3) or RATCHET_ARTIFACT_DIR (local directory)');
          })());

  // secrets from SSM: model API keys and the repo token
  const secrets: Record<string, string> = {};
  for (const [name, ref] of Object.entries(parseSecretRefs(env['RATCHET_SECRET_REFS']))) secrets[name] = await fetchSecret(spawner, baseEnv, name, ref);

  const rawConfig =
    env['RATCHET_RUNNER_CONFIG'] ??
    (env['RATCHET_RUNNER_CONFIG_FILE'] ? await readFile(env['RATCHET_RUNNER_CONFIG_FILE'], 'utf8') : await store.get(configKey(prefix)));
  if (!rawConfig) throw new Error(`no runner config: set RATCHET_RUNNER_CONFIG[_FILE] or put ${configKey(prefix)} in the artifacts store`);
  const config = validateConfig(JSON.parse(rawConfig));
  if (config.optimizationId !== optimizationId || config.agentId !== agentId) throw new Error('runner config does not match RATCHET_OPTIMIZATION_ID / RATCHET_AGENT_ID');

  const gitEnv = secrets['GITHUB_TOKEN'] ? gitAuthEnv(config.repoUrl, secrets['GITHUB_TOKEN']) : {};
  const agentEnv: Record<string, string> = {};
  for (const k of AGENT_ENV_PASSTHROUGH) if (baseEnv[k] !== undefined) agentEnv[k] = baseEnv[k];
  for (const [k, v] of Object.entries(secrets)) if (k !== 'GITHUB_TOKEN') agentEnv[k] = v;

  const workdir = env['RATCHET_WORKDIR'] ?? join(env['RATCHET_WORK_ROOT'] ?? tmpdir(), 'ratchet', optimizationId, agentId);
  const git = options.git ?? execGit;
  await prepareWorkdir({ repoUrl: config.repoUrl, branch: config.branch, baseRef: config.baseRef, dir: workdir, git, env: gitEnv });

  const adapter = options.adapterFactory?.(config, agentEnv) ?? defaultAdapter(config, env, agentEnv, spawner, clock);
  const sink = new ArtifactEventSink(store, prefix, { flushIntervalMs: Number(env['RATCHET_EVENT_FLUSH_MS'] ?? 2000) });
  const inbox = new ArtifactInbox(store, prefix, clock, Number(env['RATCHET_INBOX_POLL_MS'] ?? 2000));

  log(`ratchet-runner ${optimizationId}/${agentId} kind=${config.kind} workdir=${workdir}`);
  let result: RunnerLoopResult;
  try {
    result = await runRunnerLoop({
      config,
      adapter,
      workdir,
      inbox,
      emit: sink.emit,
      clock,
      git,
      gitEnv,
      ...(options.signal ? { signal: options.signal } : {}),
      ...(env['RATCHET_BEST_REF'] ? { bestRef: env['RATCHET_BEST_REF'] } : {}),
      ...(env['RATCHET_MODEL'] ? { model: env['RATCHET_MODEL'] } : {}),
      ...(env['RATCHET_MAX_IDLE_TURNS'] ? { maxIdleTurns: Number(env['RATCHET_MAX_IDLE_TURNS']) } : {}),
      agentEnv: {},
    });
  } finally {
    await sink.close();
  }
  await store.put(doneKey(prefix), JSON.stringify({ ...result, finishedAt: clock.now() }));
  log(`ratchet-runner finished: ${result.reason} (${result.candidates} candidates, $${result.spendUsd.toFixed(2)})`);
  return result.reason === 'failed' || result.reason === 'agent-unavailable' ? 1 : 0;
}
