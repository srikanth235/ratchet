// Shared fakes for the main-process and renderer tests: no network, no AWS, no real CLIs.
import type { HarnessInfo } from '@ratchet/agents';
import type { SandboxTeardownReceipt } from '@ratchet/core';
import type { RepoVM } from '../shared/types.js';
import type { DevicePoll, GithubApi } from './github.js';

export const GH_TOKEN = 'ghp_TOPSECRETTOKENVALUE1234567890abcd';
export const ANTHROPIC_KEY = 'sk-ant-api03-TOPSECRETANTHROPICKEY';
export const OPENAI_KEY = 'sk-proj-TOPSECRETOPENAIKEYVALUE';
export const DEVICE_TOKEN = 'gho_DEVICETOPSECRETTOKEN1234567';

/** A ratchet.yaml as a repo would declare it: a template, a container, a machine, a region and one extra protected glob. */
export const YAML_OK = `version: 0
objective: { name: Checkout latency, metric: http_req_duration.p95, direction: minimize }
evaluator: { template: http-latency }
environment: { image: .devcontainer/Dockerfile, machine: c7i.4xlarge, region: eu-west-2 }
protected: [docs/**]
`;

export class FakeGithub implements GithubApi {
  clientIdConfigured = true;
  repoList: RepoVM[] = [
    { fullName: 'acme/api', defaultBranch: 'main', private: true, description: 'checkout service', pushedAt: '2026-01-02T00:00:00Z' },
    { fullName: 'acme/web', defaultBranch: 'trunk', private: false, description: null, pushedAt: '2026-01-01T00:00:00Z' },
  ];
  files = new Map<string, string>();
  polls: DevicePoll[] = [];
  login = 'octocat';
  userFails = false;
  tokensSeen: string[] = [];
  async deviceStart() {
    return { deviceCode: 'dc', userCode: 'ABCD-1234', verificationUri: 'https://github.com/login/device', expiresIn: 900, interval: 5 };
  }
  async devicePoll(): Promise<DevicePoll> {
    return this.polls.shift() ?? { status: 'pending' };
  }
  async user(token: string) {
    this.tokensSeen.push(token);
    if (this.userFails) throw new Error('GitHub rejected the sign-in (401): sign in again in Settings.');
    return { login: this.login };
  }
  async repos(token: string, query?: string) {
    this.tokensSeen.push(token);
    const q = query?.toLowerCase();
    return q ? this.repoList.filter((r) => r.fullName.includes(q)) : this.repoList;
  }
  async repo(token: string, full: string) {
    this.tokensSeen.push(token);
    const r = this.repoList.find((x) => x.fullName === full);
    if (!r) throw new Error(`${full}: not found, or the account cannot see it (404).`);
    return r;
  }
  async file(_t: string, full: string, ref: string, path: string) {
    return this.files.get(`${full}@${ref}:${path}`) ?? null;
  }
}

export const claude = (over: Partial<HarnessInfo> = {}): HarnessInfo => ({ kind: 'claude-code', installed: true, version: '2.1.4', path: '/bin/claude', auth: 'signed-in', hint: 'signed in', ...over });
export const codex = (over: Partial<HarnessInfo> = {}): HarnessInfo => ({ kind: 'codex', installed: false, version: null, path: null, auth: 'signed-out', hint: 'Install Codex: npm i -g @openai/codex', ...over });

export const receipt = (over: Partial<SandboxTeardownReceipt> = {}): SandboxTeardownReceipt => ({
  sandboxId: 'ratchet-sandbox', region: 'us-east-1', trigger: 'idle', createdAt: 1_000, destroyedAt: Date.UTC(2026, 9, 30, 12, 0, 0),
  runs: [{ optimizationId: 'opt_demo_ingest', status: 'completed', spendUsd: 4.2 }], spendUsd: 4.2, spendByCategory: { tokens: 3, bench_runner: 1.2 },
  removed: { instances: ['i-1', 'i-2'], agentIdentities: ['a1'], secrets: ['s1'], resources: ['bucket', 'vpc'] }, nothingRunning: true, orphans: [], errors: [], summary: 'Removed 2 instances; nothing is left running.',
  ...over,
});
