import type { AgentAdapter, AgentEvent, AgentSession, AgentSpec, AgentUsage } from '@ratchet/core';
import {
  AgentAbortedError,
  AgentError,
  CliAgentBase,
  isRecord,
  num,
  truncate,
  type CliAgentOptions,
} from './cli-agent.js';
import { LineSplitter } from './process.js';

/** USD per million tokens, for estimating cost: `codex exec --json` reports tokens but no dollars. */
export interface CodexPricing {
  inputPerM: number;
  cachedInputPerM?: number;
  outputPerM: number;
}

export interface CodexTurnResult {
  completed: boolean;
  failed: boolean;
  error?: string;
  usage?: AgentUsage;
}

/**
 * Incremental parser for `codex exec --json` JSONL. Same tolerance rules as the Claude parser.
 * Usage is reported once per turn, from `turn.completed`.
 */
export class CodexStreamParser {
  threadId: string | undefined;
  turn: CodexTurnResult = { completed: false, failed: false };
  malformedLines = 0;
  readonly unknownTypes = new Set<string>();
  private readonly splitter = new LineSplitter();
  private readonly started = new Set<string>();
  private lastError: string | undefined;

  constructor(
    private readonly now: () => number = Date.now,
    private readonly pricing?: CodexPricing,
  ) {}

  push(chunk: string): AgentEvent[] {
    return this.splitter.push(chunk).flatMap((l) => this.parseLine(l));
  }

  end(): AgentEvent[] {
    return this.splitter.flush().flatMap((l) => this.parseLine(l));
  }

  parseLine(line: string): AgentEvent[] {
    let raw: unknown;
    try {
      raw = JSON.parse(line);
    } catch {
      this.malformedLines++;
      return [];
    }
    if (!isRecord(raw) || typeof raw['type'] !== 'string') {
      this.malformedLines++;
      return [];
    }
    const at = this.now();
    switch (raw['type']) {
      case 'thread.started':
        if (typeof raw['thread_id'] === 'string') this.threadId = raw['thread_id'];
        return [{ at, type: 'status', text: 'codex thread started', data: { sessionId: this.threadId } }];
      case 'turn.started':
        return [];
      case 'item.started':
      case 'item.updated':
        return isRecord(raw['item']) ? this.item(raw['item'], at, raw['type'] === 'item.started') : [];
      case 'item.completed':
        return isRecord(raw['item']) ? this.item(raw['item'], at, false) : [];
      case 'turn.completed':
        return this.turnCompleted(raw, at);
      case 'turn.failed': {
        const message = isRecord(raw['error']) && typeof raw['error']['message'] === 'string' ? raw['error']['message'] : (this.lastError ?? 'codex turn failed');
        this.turn = { completed: false, failed: true, error: message };
        return [{ at, type: 'error', text: message, data: { fatal: true } }];
      }
      case 'error': {
        // Often a transient stream error (reconnecting); `turn.failed` follows when it is fatal.
        const message = typeof raw['message'] === 'string' ? raw['message'] : 'codex error';
        this.lastError = message;
        return [{ at, type: 'error', text: message, data: { fatal: false } }];
      }
      default:
        this.unknownTypes.add(raw['type']);
        return [];
    }
  }

  private item(item: Record<string, unknown>, at: number, isStart: boolean): AgentEvent[] {
    const id = typeof item['id'] === 'string' ? item['id'] : undefined;
    const type = item['type'];
    switch (type) {
      case 'agent_message':
        return typeof item['text'] === 'string' && item['text'].trim() ? [{ at, type: 'message', text: item['text'] }] : [];
      case 'command_execution': {
        const command = typeof item['command'] === 'string' ? item['command'] : '';
        const events: AgentEvent[] = [];
        if (id && !this.started.has(id)) {
          this.started.add(id);
          events.push({ at, type: 'tool', text: `command: ${truncate(command, 200)}`, data: { phase: 'call', id, name: 'command', input: { command } } });
        }
        if (!isStart && item['status'] !== 'in_progress') {
          const exitCode = typeof item['exit_code'] === 'number' ? item['exit_code'] : undefined;
          events.push({
            at,
            type: 'tool',
            text: truncate(typeof item['aggregated_output'] === 'string' ? item['aggregated_output'] : '', 300),
            data: { phase: 'result', id, exitCode, isError: exitCode !== undefined && exitCode !== 0 },
          });
        }
        return events;
      }
      case 'file_change': {
        if (isStart) return [];
        const changes = Array.isArray(item['changes']) ? item['changes'] : [];
        const paths = changes.flatMap((c) => (isRecord(c) && typeof c['path'] === 'string' ? [c['path']] : []));
        return [{ at, type: 'tool', text: `apply_patch: ${paths.join(', ')}`, data: { phase: 'call', id, name: 'apply_patch', input: { changes } } }];
      }
      case 'mcp_tool_call':
      case 'web_search': {
        if (isStart) return [];
        const name = type === 'web_search' ? 'web_search' : `${String(item['server'] ?? 'mcp')}.${String(item['tool'] ?? '')}`;
        return [{ at, type: 'tool', text: name, data: { phase: 'call', id, name } }];
      }
      case 'error':
        return typeof item['message'] === 'string' ? [{ at, type: 'error', text: item['message'], data: { fatal: false } }] : [];
      case 'reasoning':
      case 'todo_list':
        return [];
      default:
        if (typeof type === 'string') this.unknownTypes.add(`item:${type}`);
        return [];
    }
  }

  private turnCompleted(raw: Record<string, unknown>, at: number): AgentEvent[] {
    const u = isRecord(raw['usage']) ? raw['usage'] : {};
    const cached = num(u['cached_input_tokens']);
    const usage: AgentUsage = { inputTokens: num(u['input_tokens']), outputTokens: num(u['output_tokens']) };
    if (this.pricing) {
      const p = this.pricing;
      usage.costUsd =
        ((usage.inputTokens - cached) * p.inputPerM + cached * (p.cachedInputPerM ?? p.inputPerM) + usage.outputTokens * p.outputPerM) / 1e6;
    }
    this.turn = { completed: true, failed: false, usage };
    return [{ at, type: 'usage', usage, data: { cachedInputTokens: cached } }];
  }
}

export interface CodexOptions extends CliAgentOptions {
  /** Sandbox mode passed as `-c sandbox_mode=...`. Default `workspace-write`. */
  sandbox?: 'read-only' | 'workspace-write' | 'danger-full-access';
  /** Allow network inside the workspace-write sandbox. Default true (agents run experiments, pnpm, aws). */
  networkAccess?: boolean;
  /** Extra `-c key=value` overrides. */
  configOverrides?: string[];
  pricing?: CodexPricing;
  /** Deliver the prompt on stdin via `-` (default) or as the final argument. */
  promptVia?: 'stdin' | 'arg';
  /** Set as OPENAI_API_KEY and CODEX_API_KEY for the child. */
  apiKey?: string;
  /** Skip the API-key preflight check: the CLI is signed in with a ChatGPT account (`codex login`) on this machine. */
  allowSubscriptionAuth?: boolean;
}

export class CodexAdapter extends CliAgentBase implements AgentAdapter {
  readonly kind = 'codex' as const;
  private readonly options: CodexOptions;

  constructor(options: CodexOptions = {}) {
    super('codex', options);
    this.options = options;
    if (options.apiKey) this.baseEnv = { ...this.baseEnv, OPENAI_API_KEY: options.apiKey };
  }

  async preflight(): Promise<{ ok: boolean; problems: string[] }> {
    const problems: string[] = [];
    const binaryProblem = await this.probeBinary();
    if (binaryProblem) problems.push(binaryProblem);
    if (!this.options.allowSubscriptionAuth && !this.baseEnv['OPENAI_API_KEY'] && !this.baseEnv['CODEX_API_KEY']) problems.push('OPENAI_API_KEY is not set');
    return { ok: problems.length === 0, problems };
  }

  /** Argument list for one invocation, excluding the prompt (exported for tests and docs). */
  buildArgs(spec: AgentSpec, resumeId?: string): string[] {
    const o = this.options;
    const args = ['exec'];
    if (resumeId) args.push('resume', resumeId);
    args.push('--json', '--skip-git-repo-check');
    if (spec.model) args.push('--model', spec.model);
    // Config overrides rather than --sandbox/--cd: they are accepted by `exec` and `exec resume` alike.
    const sandbox = o.sandbox ?? 'workspace-write';
    args.push('-c', `sandbox_mode="${sandbox}"`);
    if (sandbox === 'workspace-write' && (o.networkAccess ?? true)) args.push('-c', 'sandbox_workspace_write.network_access=true');
    for (const override of o.configOverrides ?? []) args.push('-c', override);
    args.push(...this.extraArgs);
    return args;
  }

  async start(spec: AgentSpec, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<AgentSession> {
    const session: AgentSession = { id: '', kind: 'codex' };
    await this.turn(session, this.buildArgs(spec), spec, spec.prompt, onEvent, signal, true);
    return session;
  }

  async resume(
    session: AgentSession,
    spec: AgentSpec,
    message: string,
    onEvent: (e: AgentEvent) => void,
    signal?: AbortSignal,
  ): Promise<void> {
    await this.turn(session, this.buildArgs(spec, session.id), spec, message, onEvent, signal, false);
  }

  private async turn(
    session: AgentSession,
    args: string[],
    spec: AgentSpec,
    prompt: string,
    onEvent: (e: AgentEvent) => void,
    signal: AbortSignal | undefined,
    isStart: boolean,
  ): Promise<void> {
    const parser = new CodexStreamParser(() => this.clock.now(), this.options.pricing);
    const viaArg = this.options.promptVia === 'arg';
    const finalArgs = viaArg ? [...args, '--', prompt] : [...args, '-'];
    const env = { ...spec.env };
    const key = this.baseEnv['OPENAI_API_KEY'];
    if (key && !this.baseEnv['CODEX_API_KEY']) env['CODEX_API_KEY'] = key;
    const res = await this.run(
      finalArgs,
      { ...spec, env },
      viaArg ? undefined : prompt,
      (line) => {
        for (const e of parser.parseLine(line)) onEvent(e);
      },
      signal,
    );
    if (parser.threadId) session.id = parser.threadId;

    const fail = (message: string): never => {
      onEvent({ at: this.clock.now(), type: 'error', text: message, data: { exitCode: res.code, stderr: res.stderrTail } });
      throw new AgentError(message, { exitCode: res.code, stderr: res.stderrTail });
    };

    if (res.aborted) {
      onEvent({ at: this.clock.now(), type: 'status', text: 'codex turn aborted' });
      throw new AgentAbortedError(session.id ? session : undefined);
    }
    if (res.signal?.startsWith('spawn-error')) fail(`cannot run '${this.binary}': ${res.signal.slice('spawn-error: '.length)}`);
    if (parser.turn.failed) throw new AgentError(parser.turn.error ?? 'codex turn failed', { exitCode: res.code, stderr: res.stderrTail });
    if (!parser.turn.completed) {
      fail(
        res.code !== 0
          ? `codex exited with ${res.signal ?? `code ${res.code}`}${res.stderrTail ? `: ${res.stderrTail}` : ''}`
          : 'codex stream ended without turn.completed',
      );
    }
    if (isStart && !session.id) fail('codex did not report a thread id');
  }
}
