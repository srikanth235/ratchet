import { tokens } from '../tokens.js';

export type AgentKind = 'claude-code' | 'codex';

const AGENTS: Record<AgentKind, { initials: string; color: string; label: string }> = {
  'claude-code': { initials: 'CC', color: tokens.color.claudeCode, label: 'Claude Code' },
  codex: { initials: 'CX', color: tokens.color.codex, label: 'Codex' },
};

export function AgentAvatar({ agent, size = 24 }: { agent: AgentKind; size?: number }) {
  const a = AGENTS[agent];
  return (
    <span
      className="rt-avatar"
      role="img"
      aria-label={a.label}
      title={a.label}
      style={{ width: size, height: size, background: a.color, fontSize: Math.round(size * 0.4) }}
    >
      {a.initials}
    </span>
  );
}
