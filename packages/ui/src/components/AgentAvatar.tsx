import { tokens } from '../tokens.js';

export type AgentKind = 'claude-code' | 'codex' | 'mock';

const AGENTS: Record<AgentKind, { initials: string; color: string; bg: string; ink: string; label: string }> = {
  'claude-code': { initials: 'CC', color: tokens.color.claudeCode, bg: tokens.color.claudeCodeSoft, ink: tokens.color.claudeCodeInk, label: 'Claude Code' },
  codex: { initials: 'CX', color: tokens.color.codex, bg: tokens.color.codexSoft, ink: tokens.color.codexInk, label: 'Codex' },
  mock: { initials: 'MK', color: tokens.color.textTertiary, bg: tokens.color.raised, ink: tokens.color.textSecondary, label: 'Mock agent' },
};

export interface AgentAvatarProps {
  agent: AgentKind;
  /** Disc diameter in px (a ring adds 2×2px around it). */
  size?: number;
  /** Card-coloured ring, for stacked avatars. */
  ring?: boolean;
  /** Overlap with the previous avatar (px, negative margin-left). */
  overlap?: number;
}

/** Initials font size that tracks the disc, matching the design's 9 / 10 / 12 / 13px steps. */
function initialsSize(size: number): number {
  if (size <= 24) return 9;
  if (size <= 30) return 10;
  if (size <= 38) return 12;
  return 13;
}

export function AgentAvatar({ agent, size = 24, ring = false, overlap = 0 }: AgentAvatarProps) {
  const a = AGENTS[agent];
  return (
    <span
      className={`rt-avatar${ring ? ' rt-avatar--ring' : ''}`}
      role="img"
      aria-label={a.label}
      title={a.label}
      style={{
        width: size,
        height: size,
        background: a.bg,
        color: a.ink,
        fontSize: initialsSize(size),
        ...(overlap ? { marginLeft: -overlap } : {}),
      }}
    >
      {a.initials}
    </span>
  );
}

/** Chart / bar colour for an agent kind (brighter than the avatar disc). */
export function agentColor(agent: AgentKind): string {
  return AGENTS[agent].color;
}

/** Avatar disc colours for an agent kind. */
export function agentSoft(agent: AgentKind): { bg: string; ink: string } {
  return { bg: AGENTS[agent].bg, ink: AGENTS[agent].ink };
}

export function agentLabel(agent: AgentKind): string {
  return AGENTS[agent].label;
}
