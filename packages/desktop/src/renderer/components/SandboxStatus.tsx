import { useEffect, useRef, useState } from 'react';
import type { EnvironmentVM, EnvironmentsVM, StatusTone } from '../../shared/types';
import { errMsg, useNav, useQuery } from '../data';
import { fmtUsd } from '../format';
import { StatusDot } from './OptimizationsTable';
import { Receipt } from './Receipt';

const clock = (ms: number): string => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false });

const PHASE: Record<EnvironmentVM['phase'], { word: string; tone: StatusTone }> = {
  creating: { word: 'Sandbox starting', tone: 'run' },
  ready: { word: 'Sandbox on', tone: 'ok' },
  idle: { word: 'Sandbox idle', tone: 'ok' },
  destroying: { word: 'Sandbox shutting down', tone: 'run' },
  destroyed: { word: 'Sandbox off', tone: 'off' },
  failed: { word: 'Sandbox failed', tone: 'bad' },
  none: { word: 'No sandbox running', tone: 'off' },
};

/** `$1.20 so far · off at 15:40` */
function detail(env: EnvironmentVM): string {
  const parts = [`${fmtUsd(env.spendUsd)} so far`];
  if (env.destroyAt) parts.push(`off at ${clock(env.destroyAt)}`);
  else if (env.activeRuns.length > 0) parts.push('off after the last run');
  return parts.join(' · ');
}

function Panel({ data, busy, error, onEnd }: { data: EnvironmentsVM; busy: boolean; error: string | null; onEnd(id: string): void }) {
  const { go } = useNav();
  const [confirm, setConfirm] = useState<string | null>(null);
  const live = data.environments;
  const receipt = data.lastReceipt;
  return (
    <div className="sbx-panel" role="dialog" aria-label="Sandbox" data-testid="sandbox-panel">
      {live.length === 0 ? (
        <p className="sbx-panel__text" data-testid="no-environment">No sandbox is running, so nothing is billing. One starts by itself when a run begins.</p>
      ) : (
        live.map((env) => (
          <div key={env.sandboxId} className="sbx-panel__env" data-testid="environment" data-phase={env.phase}>
            <div className="sbx-panel__head">
              <span className="mono">{env.sandboxId}</span>
              <span className="faint">{env.region ?? 'region pending'}{env.simulated ? ' · simulated' : ''}</span>
            </div>
            <p className="sbx-panel__text">{env.text}</p>
            <dl className="sbx-panel__facts">
              <div><dt>Runs active</dt><dd className="mono">{env.activeRuns.length}</dd></div>
              <div><dt>Instances</dt><dd className="mono">{env.instancesRunning ?? '—'}</dd></div>
              <div><dt>Cost so far</dt><dd className="mono">{fmtUsd(env.spendUsd)}</dd></div>
            </dl>
            <p className="sbx-panel__text">
              {env.destroyAt ? <>Removes itself at <span className="mono">{clock(env.destroyAt)}</span>, {data.idleMinutes} min after the last run ended.</> : <>Removed automatically {data.idleMinutes} min after the last run ends.</>}
            </p>
            {confirm === env.sandboxId ? (
              <div role="group" aria-label="Confirm end sandbox" className="sbx-confirm">
                <p className="sbx-panel__text">End {env.sandboxId} now? Ratchet tears down the finished runs’ cloud resources, destroys the sandbox and checks that nothing is left running. The next run starts a new one.</p>
                <div className="sbx-panel__actions">
                  <button type="button" className="btn-sm btn-sm--danger" disabled={busy} onClick={() => { setConfirm(null); onEnd(env.sandboxId); }}>Confirm end sandbox</button>
                  <button type="button" className="btn-sm" disabled={busy} onClick={() => setConfirm(null)}>Cancel</button>
                </div>
              </div>
            ) : (
              <div className="sbx-panel__actions">
                <button type="button" className="btn-sm btn-sm--danger" disabled={busy || !env.canEnd} title={env.canEnd ? undefined : 'A run is active in this sandbox'} onClick={() => setConfirm(env.sandboxId)}>End now</button>
                {!env.canEnd && env.activeRuns.length > 0 && <span className="faint">Available once the active runs have ended.</span>}
              </div>
            )}
          </div>
        ))
      )}
      {error && <p role="alert" className="error-note">{error}</p>}
      {receipt && live.length === 0 && (
        <div className="sbx-panel__receipt">
          <h3>Last teardown</h3>
          <Receipt r={receipt} compact />
        </div>
      )}
      <button type="button" className="text-link sbx-panel__more" onClick={() => go('sandbox')}>Details</button>
    </div>
  );
}

/**
 * The quiet sandbox line at the bottom of the sidebar ("● Sandbox on · acme-prod / $1.20 so far · off at 15:40").
 * Opens a small panel with what is running and "End now" (behind a confirmation), then the receipt it produced.
 */
export function SandboxStatus() {
  const q = useQuery(() => window.ratchet.environments(), [], { intervalMs: 10_000, relevant: (e) => e.type === 'sandbox' || e.type === 'run-active' });
  const [local, setLocal] = useState<EnvironmentsVM | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  // The answer to "End now" is shown at once; the next load of the query takes over.
  const data = local ?? q.data;
  useEffect(() => setLocal(null), [q.data]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    const onDown = (e: MouseEvent) => root.current && !root.current.contains(e.target as Node) && setOpen(false);
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onDown);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
    };
  }, [open]);

  const end = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      setLocal(await window.ratchet.endSandbox(id));
      q.refresh();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  };

  const env = data?.environments[0];
  const p = PHASE[env?.phase ?? 'none'];
  return (
    <div className="sbx" ref={root} data-testid="sandbox-status">
      {open && data && <Panel data={data} busy={busy} error={error} onEnd={(id) => void end(id)} />}
      <button type="button" className="sbx__line" aria-expanded={open} aria-haspopup="dialog" onClick={() => setOpen((v) => !v)}>
        <span className="sbx__row"><StatusDot tone={p.tone} />{env ? `${p.word} · ${env.sandboxId}` : p.word}</span>
        {env && <span className="sbx__row sbx__row--sub">{detail(env)}</span>}
      </button>
    </div>
  );
}
