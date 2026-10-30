import { useState } from 'react';
import { Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import type { EnvironmentVM, EnvironmentsVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { Receipt } from '../components/Receipt';
import { errMsg, useQuery } from '../data';
import { fmtUsd } from '../format';

const BOX = { fill: '#151D2B', stroke: 'rgba(148,163,190,.2)' };
const TRUSTED = { fill: '#1A1A16', stroke: 'rgba(217,178,111,.55)' };

function Node({ x, y, title, sub, trusted = false, icon }: { x: number; y: number; title: string; sub: string; trusted?: boolean; icon: string }) {
  const c = trusted ? TRUSTED : BOX;
  return (
    <g>
      <rect x={x} y={y} width="176" height="72" rx="12" fill={c.fill} stroke={c.stroke} />
      <path d={icon} transform={`translate(${x + 14} ${y + 28})`} fill="none" stroke={trusted ? '#D9B26F' : '#A7B1C2'} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      <text x={x + 44} y={y + 32} fontSize="12.5" fontWeight="600" fill={trusted ? '#F4E3C0' : '#ECEFF4'} fontFamily="Instrument Sans, sans-serif">{title}</text>
      <text x={x + 44} y={y + 49} fontSize="12" fill={trusted ? '#A7B1C2' : '#8592A6'} fontFamily="Instrument Sans, sans-serif">{sub}</text>
    </g>
  );
}

/** Architecture sketch: the trusted path (brass) from the control plane through the evaluator runner to the sealed traces. */
export function SandboxDiagram({ region, sandboxId }: { region: string; sandboxId: string }) {
  const grey = '#8592A6';
  const label = { fontSize: 13, fontFamily: 'Instrument Sans, sans-serif' } as const;
  return (
    <div className="diagram rt-dots" style={{ padding: 0 }}>
      <svg viewBox="0 0 1016 340" role="img" aria-label="Sandbox architecture: your Mac runs the control plane and the agents; the evaluator runner and sealed traces sit on the trusted path; optional cloud runners are confined to the agent boundary" className="rt-chart sandbox-diagram">
        <defs>
          <marker id="sb-grey" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0L8 4L0 8z" fill={grey} /></marker>
          <marker id="sb-brass" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0L8 4L0 8z" fill="#D9B26F" /></marker>
        </defs>
        <rect x="264" y="168" width="744" height="156" rx="14" fill="rgba(217,178,111,.04)" stroke="rgba(217,178,111,.28)" strokeDasharray="5 5" />
        <path d="M112 112 V232 H288" fill="none" stroke={grey} strokeWidth="1.5" markerEnd="url(#sb-grey)" />
        <path d="M376 196 V76 H552" fill="none" stroke={grey} strokeWidth="1.5" markerEnd="url(#sb-grey)" />
        <path d="M640 112 V196" fill="none" stroke="#D9B26F" strokeWidth="2" markerEnd="url(#sb-brass)" />
        <path d="M816 232 H728" fill="none" stroke="#D9B26F" strokeWidth="2" markerEnd="url(#sb-brass)" />
        <Node x={24} y={40} title="Ratchet.app" sub="your Mac · agents" icon="M0 0h14v9H0zM4 13h6" />
        <Node x={552} y={40} title="GitHub" sub="one branch per agent" icon="M3 0a2 2 0 1 0 0 .01M3 12a2 2 0 1 0 0 .01M11 4a2 2 0 1 0 0 .01M3 2v8M11 6c0 4-8 2-8 4" />
        <Node x={288} y={196} title="Cloud runners" sub="optional · unattended" icon="M0 0h14v10H0zM3 3l2 2-2 2M7 8h3" />
        <Node x={552} y={196} title="Evaluator runner" sub="eval-only role" trusted icon="M0 10a7 7 0 0 1 14 0M7 10l3-4" />
        <Node x={816} y={196} title="Sealed traces" sub="S3 · deny agents" trusted icon="M2 6h10v7H2zM4 6V4a3 3 0 0 1 6 0v2" />
        <text x="124" y="176" fill="#A7B1C2" {...label}>unattended · SSM</text>
        <text x="448" y="62" fill="#A7B1C2" {...label}>push</text>
        <text x="392" y="140" fill="#A7B1C2" {...label}>cloud agents</text>
        <text x="656" y="158" fill="#EBCB8E" {...label}>re-evaluate</text>
        <text x="756" y="216" fill="#EBCB8E" {...label}>read</text>
        <text x="284" y="310" fill="#C9A86A" fontSize="10.5" letterSpacing="1.5" fontWeight="600" fontFamily="Instrument Sans, sans-serif">YOUR AWS ACCOUNT · {region.toUpperCase()} · {sandboxId.toUpperCase()}</text>
      </svg>
    </div>
  );
}

const PHASE: Record<EnvironmentVM['phase'], { tone: 'mint' | 'brass' | 'rose' | 'neutral'; label: string; live?: boolean }> = {
  creating: { tone: 'brass', label: 'Creating', live: true },
  ready: { tone: 'mint', label: 'Ready' },
  idle: { tone: 'neutral', label: 'Idle' },
  destroying: { tone: 'brass', label: 'Removing', live: true },
  destroyed: { tone: 'neutral', label: 'Removed' },
  failed: { tone: 'rose', label: 'Failed' },
  none: { tone: 'neutral', label: 'None' },
};

const clock = (ms: number): string => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

function EnvironmentCard({ env, idleMinutes, busy, onEnd }: { env: EnvironmentVM; idleMinutes: number; busy: boolean; onEnd(): void }) {
  const p = PHASE[env.phase];
  const [confirm, setConfirm] = useState(false);
  return (
    <Card className="env-card" data-testid="environment" data-phase={env.phase}>
      <div className="env-card__head">
        <div className="env-card__id">
          <span className="mono env-card__name">{env.sandboxId}</span>
          <span className="faint mono">{env.region ?? 'region pending'}{env.simulated ? ' · simulated' : ''}</span>
        </div>
        <Pill tone={p.tone} dot live={p.live} large>{p.label}</Pill>
      </div>
      <p className="muted env-card__text">{env.text}</p>
      <div className="env-card__stats">
        <div className="limit-tile"><div className="limit-tile__label">Runs active</div><div className="serif">{env.activeRuns.length}</div></div>
        <div className="limit-tile"><div className="limit-tile__label">Instances running</div><div className="serif">{env.instancesRunning ?? '—'}</div></div>
        <div className="limit-tile"><div className="limit-tile__label">Cost so far</div><div className="serif">{fmtUsd(env.spendUsd)}</div></div>
      </div>
      <p className="muted env-card__removal">
        {env.destroyAt
          ? <>Removes itself at <span className="mono">{clock(env.destroyAt)}</span>, {idleMinutes} min after the last run ended.</>
          : env.activeRuns.length > 0
            ? <>Removed automatically {idleMinutes} min after the last run ends.</>
            : <>Removed automatically after {idleMinutes} idle minutes.</>}
      </p>
      {confirm ? (
        <div role="group" aria-label="Confirm end sandbox" className="confirm-card confirm-card--danger env-card__confirm">
          <h2 className="confirm-card__title">End {env.sandboxId} now?</h2>
          <p className="confirm-card__body">Ratchet tears down the finished runs’ cloud resources, destroys the environment and checks that nothing is left running. The next run creates a new one.</p>
          <div className="confirm-card__actions">
            <Button variant="danger" disabled={busy} onClick={() => { setConfirm(false); onEnd(); }}>Confirm end sandbox</Button>
            <Button variant="ghost" disabled={busy} onClick={() => setConfirm(false)}>Cancel</Button>
          </div>
        </div>
      ) : (
        <div className="env-card__actions">
          <Button variant="danger-outline" disabled={busy || !env.canEnd} title={env.canEnd ? undefined : 'A run is active in this environment'} onClick={() => setConfirm(true)}>End sandbox now</Button>
          {!env.canEnd && env.activeRuns.length > 0 && <span className="muted">Available once the active runs have ended.</span>}
        </div>
      )}
    </Card>
  );
}

export function Sandbox() {
  const q = useQuery(() => window.ratchet.environments(), [], { intervalMs: 10_000, relevant: (e) => e.type === 'sandbox' || e.type === 'run-active' });
  const [local, setLocal] = useState<EnvironmentsVM | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const data = q.data ?? local;
  if (q.error && !data) return <ErrorNote error={q.error} />;
  if (!data) return <p className="muted">Loading…</p>;

  const live = data.environments;
  const first = live[0];
  const end = async (id: string) => {
    setBusy(true);
    setActionError(null);
    try {
      setLocal(await window.ratchet.endSandbox(id));
      q.refresh();
    } catch (e) {
      setActionError(errMsg(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHeader
        eyebrow="Environments"
        title="Environments"
        badge={first ? <Pill tone={PHASE[first.phase].tone} dot live={PHASE[first.phase].live} large>{PHASE[first.phase].label}</Pill> : <Pill tone="neutral" dot large>None running</Pill>}
        sub="The evaluator’s sandbox in your AWS account. Ratchet creates it when a run starts and removes it after the last run plus an idle window: nothing to apply or destroy."
      />
      <ErrorNote error={actionError} />

      {live.length === 0 ? (
        <Card className="card-pad" data-testid="no-environment">
          <Eyebrow>Running now</Eyebrow>
          <p className="muted">No environment is running, so nothing is billing. One is created automatically when a run starts.</p>
        </Card>
      ) : (
        <section className="env-list" aria-label="Running environments">
          {live.map((env) => <EnvironmentCard key={env.sandboxId} env={env} idleMinutes={data.idleMinutes} busy={busy} onEnd={() => void end(env.sandboxId)} />)}
        </section>
      )}

      <Card className="card-pad" data-testid="last-receipt">
        <div className="card-head">
          <h2 className="card-title">Last teardown receipt</h2>
          <span className="card-head__note">Written every time an environment is removed.</span>
        </div>
        {data.lastReceipt ? <Receipt r={data.lastReceipt} /> : <p className="muted">No environment has been removed yet.</p>}
      </Card>

      <Card className="diagram-card">
        <div className="card-head">
          <h2 className="card-title">How it fits together</h2>
          <span className="card-head__note">Brass lines are the trusted path agents can’t touch.</span>
        </div>
        <SandboxDiagram region={first?.region ?? 'us-east-1'} sandboxId={first?.sandboxId ?? 'ratchet-sandbox'} />
      </Card>
    </>
  );
}
