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

export interface ClaudeResult {
  subtype: string;
  isError: boolean;
  text: string;
  numTurns: number;
  durationMs: number;
  usage: AgentUsage;
}

function toolSummary(name: string, input: unknown): string {
  if (isRecord(input)) {
    for (const key of ['command', 'file_path', 'path', 'pattern', 'url', 'description']) {
      const v = input[key];
      if (typeof v === 'string') return `${name}: ${truncate(v, 200)}`;
    }
    return `${name}: ${truncate(JSON.stringify(input), 200)}`;
  }
  return name;
}

function blockText(content: unknown): string {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.map((b) => (isRecord(b) && typeof b['text'] === 'string' ? b['text'] : '')).filter(Boolean).join('\n');
  }
  return '';
}

/**
 * Incremental parser for `claude -p --output-format stream-json --verbose` output.
 * Tolerant by design: partial lines are buffered, non-JSON lines and unknown event/block types are
 * counted and skipped, never thrown. Usage is reported once per turn, from the `result` line.
 */
export class ClaudeStreamParser {
  sessionId: string | undefined;
  model: string | undefined;
  result: ClaudeResult | undefined;
  malformedLines = 0;
  readonly unknownTypes = new Set<string>();
  private readonly splitter = new LineSplitter();

  constructor(private readonly now: () => number = Date.now) {}

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
    if (typeof raw['session_id'] === 'string' && !this.sessionId) this.sessionId = raw['session_id'];
    const at = this.now();
    switch (raw['type']) {
      case 'system':
        return this.system(raw, at);
      case 'assistant':
        return this.assistant(raw, at);
      case 'user':
        return this.user(raw, at);
      case 'result':
        return this.resultLine(raw, at);
      default:
        this.unknownTypes.add(raw['type']);
        return [];
    }
  }

  private system(raw: Record<string, unknown>, at: number): AgentEvent[] {
    const subtype = raw['subtype'];
    if (subtype === 'init') {
      if (typeof raw['session_id'] === 'string') this.sessionId = raw['session_id'];
      if (typeof raw['model'] === 'string') this.model = raw['model'];
      return [
        {
          at,
          type: 'status',
          text: `claude-code session started${this.model ? ` (${this.model})` : ''}`,
          data: {
            sessionId: this.sessionId,
            model: this.model,
            permissionMode: raw['permissionMode'],
            apiKeySource: raw['apiKeySource'],
            tools: Array.isArray(raw['tools']) ? raw['tools'].length : undefined,
          },
        },
      ];
    }
    if (subtype === 'api_retry' || subtype === 'compact_boundary') {
      return [{ at, type: 'status', text: `claude-code ${String(subtype)}`, data: { ...raw } }];
    }
    if (typeof subtype === 'string') this.unknownTypes.add(`system:${subtype}`);
    return [];
  }

  private assistant(raw: Record<string, unknown>, at: number): AgentEvent[] {
    const message = raw['message'];
    const content = isRecord(message) ? message['content'] : undefined;
    if (!Array.isArray(content)) return [];
    const events: AgentEvent[] = [];
    for (const block of content) {
      if (!isRecord(block)) continue;
      if (block['type'] === 'text' && typeof block['text'] === 'string' && block['text'].trim()) {
        events.push({ at, type: 'message', text: block['text'] });
      } else if (block['type'] === 'tool_use') {
        const name = typeof block['name'] === 'string' ? block['name'] : 'unknown';
        events.push({
          at,
          type: 'tool',
          text: toolSummary(name, block['input']),
          data: { phase: 'call', id: block['id'], name, input: block['input'] },
        });
      } else if (typeof block['type'] === 'string' && block['type'] !== 'thinking' && block['type'] !== 'text') {
        this.unknownTypes.add(`block:${block['type']}`);
      }
    }
    return events;
  }

  private user(raw: Record<string, unknown>, at: number): AgentEvent[] {
    const message = raw['message'];
    const content = isRecord(message) ? message['content'] : undefined;
    if (!Array.isArray(content)) return [];
    const events: AgentEvent[] = [];
    for (const block of content) {
      if (!isRecord(block) || block['type'] !== 'tool_result') continue;
      const isError = block['is_error'] === true;
      events.push({
        at,
        type: 'tool',
        text: truncate(blockText(block['content']), 300),
        data: { phase: 'result', id: block['tool_use_id'], isError },
      });
    }
    return events;
  }

  private resultLine(raw: Record<string, unknown>, at: number): AgentEvent[] {
    const u = isRecord(raw['usage']) ? raw['usage'] : {};
    const cacheCreate = num(u['cache_creation_input_tokens']);
    const cacheRead = num(u['cache_read_input_tokens']);
    const usage: AgentUsage = {
      inputTokens: num(u['input_tokens']) + cacheCreate + cacheRead,
      outputTokens: num(u['output_tokens']),
    };
    if (typeof raw['total_cost_usd'] === 'number') usage.costUsd = raw['total_cost_usd'];
    else if (typeof raw['cost_usd'] === 'number') usage.costUsd = raw['cost_usd'];
    const subtype = typeof raw['subtype'] === 'string' ? raw['subtype'] : 'success';
    const isError = raw['is_error'] === true || subtype.startsWith('error');
    this.result = {
      subtype,
      isError,
      text: typeof raw['result'] === 'string' ? raw['result'] : '',
      numTurns: num(raw['num_turns']),
      durationMs: num(raw['duration_ms']),
      usage,
    };
    const events: AgentEvent[] = [
      {
        at,
        type: 'usage',
        usage,
        data: { cacheCreationInputTokens: cacheCreate, cacheReadInputTokens: cacheRead, numTurns: this.result.numTurns },
      },
    ];
    if (isError) {
      events.push({
        at,
        type: 'error',
        text: this.result.text || `claude-code turn ended with ${subtype}`,
        data: { subtype },
      });
    }
    return events;
  }
}

export type ClaudePermissionMode = 'default' | 'acceptEdits' | 'bypassPermissions' | 'plan';

export interface ClaudeCodeOptions extends CliAgentOptions {
  /** `--permission-mode`. Default `acceptEdits`. */
  permissionMode?: ClaudePermissionMode;
  /** `--allowedTools` (comma-joined into one value). Default: the file tools plus Bash. */
  allowedTools?: string[];
  disallowedTools?: string[];
  appendSystemPrompt?: string;
  /** Deliver the prompt on stdin (default) or as the final argument. */
  promptVia?: 'stdin' | 'arg';
  /** Set as ANTHROPIC_API_KEY for the child (otherwise inherited from `env`). */
  apiKey?: string;
  /** Skip the API-key preflight check (subscription login on the box; see docs/real-env/agents.md on terms). */
  allowSubscriptionAuth?: boolean;
}

export const DEFAULT_CLAUDE_TOOLS = ['Bash', 'Read', 'Edit', 'Write', 'Glob', 'Grep'];

export class ClaudeCodeAdapter extends CliAgentBase implements AgentAdapter {
  readonly kind = 'claude-code' as const;
  private readonly options: ClaudeCodeOptions;

  constructor(options: ClaudeCodeOptions = {}) {
    super('claude', options);
    this.options = options;
    if (options.apiKey) this.baseEnv = { ...this.baseEnv, ANTHROPIC_API_KEY: options.apiKey };
  }

  async preflight(): Promise<{ ok: boolean; problems: string[] }> {
    const problems: string[] = [];
    const binaryProblem = await this.probeBinary();
    if (binaryProblem) problems.push(binaryProblem);
    if (!this.options.allowSubscriptionAuth && !this.baseEnv['ANTHROPIC_API_KEY']) {
      problems.push('ANTHROPIC_API_KEY is not set');
    }
    return { ok: problems.length === 0, problems };
  }

  /** Argument list for one invocation (exported for tests and docs). */
  buildArgs(spec: AgentSpec, resumeId?: string): string[] {
    const o = this.options;
    const args = ['-p', '--output-format', 'stream-json', '--verbose'];
    args.push('--permission-mode', o.permissionMode ?? 'acceptEdits');
    const allowed = o.allowedTools ?? DEFAULT_CLAUDE_TOOLS;
    if (allowed.length) args.push('--allowedTools', allowed.join(','));
    if (o.disallowedTools?.length) args.push('--disallowedTools', o.disallowedTools.join(','));
    if (spec.model) args.push('--model', spec.model);
    if (spec.maxTurns) args.push('--max-turns', String(spec.maxTurns));
    if (o.appendSystemPrompt) args.push('--append-system-prompt', o.appendSystemPrompt);
    if (resumeId) args.push('--resume', resumeId);
    args.push(...this.extraArgs);
    return args;
  }

  async start(spec: AgentSpec, onEvent: (e: AgentEvent) => void, signal?: AbortSignal): Promise<AgentSession> {
    const session: AgentSession = { id: '', kind: 'claude-code' };
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
    const parser = new ClaudeStreamParser(() => this.clock.now());
    const viaArg = this.options.promptVia === 'arg';
    const finalArgs = viaArg ? [...args, '--', prompt] : args;
    const res = await this.run(
      finalArgs,
      spec,
      viaArg ? undefined : prompt,
      (line) => {
        for (const e of parser.parseLine(line)) onEvent(e);
      },
      signal,
    );
    // A resumed session may come back under a new id; keep the handle current.
    if (parser.sessionId) session.id = parser.sessionId;

    const fail = (message: string): never => {
      onEvent({ at: this.clock.now(), type: 'error', text: message, data: { exitCode: res.code, stderr: res.stderrTail } });
      throw new AgentError(message, { exitCode: res.code, stderr: res.stderrTail });
    };

    if (res.aborted) {
      onEvent({ at: this.clock.now(), type: 'status', text: 'claude-code turn aborted' });
      throw new AgentAbortedError(session.id ? session : undefined);
    }
    if (res.signal?.startsWith('spawn-error')) fail(`cannot run '${this.binary}': ${res.signal.slice('spawn-error: '.length)}`);
    if (parser.result?.isError && parser.result.subtype !== 'error_max_turns') {
      // the parser already emitted the error event
      throw new AgentError(parser.result.text || `claude-code ended with ${parser.result.subtype}`, {
        exitCode: res.code,
        stderr: res.stderrTail,
      });
    }
    if (!parser.result) {
      fail(
        res.code !== 0
          ? `claude exited with ${res.signal ?? `code ${res.code}`}${res.stderrTail ? `: ${res.stderrTail}` : ''}`
          : 'claude stream ended without a result line',
      );
    }
    if (isStart && !session.id) fail('claude did not report a session id');
  }
}
