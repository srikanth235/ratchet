import { Eyebrow, Pill } from '@ratchet/ui';
import type { TeardownReceiptVM } from '../../shared/types';
import { fmtUsd } from '../format';
import { Icon } from './icons';

const when = (ms: number): string => new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
const n = (count: number, one: string, many = `${one}s`): string => `${count} ${count === 1 ? one : many}`;

/**
 * The teardown receipt: what the environment cost, what was removed, and the proof that nothing is left running.
 * Shown on Result (for the run) and on Environments (the last one).
 */
export function Receipt({ r, compact = false }: { r: TeardownReceiptVM; compact?: boolean }) {
  return (
    <div className="receipt" data-testid="receipt" aria-label="Teardown receipt">
      <div className="receipt__head">
        <Pill tone={r.nothingRunning && r.errors.length === 0 ? 'mint' : 'rose'} dot>{r.nothingRunning ? 'Nothing left running' : 'Something may still be running'}</Pill>
        <span className="faint mono">{r.region} · {r.trigger === 'manual' ? 'ended by you' : 'removed automatically'} · {when(r.destroyedAt)}</span>
      </div>
      <p className="receipt__summary">{r.summary}</p>
      <div className="receipt__grid">
        <div className="receipt__cell"><Eyebrow>Cost</Eyebrow><span className="serif receipt__num">{fmtUsd(r.spendUsd)}</span>
          {!compact && r.spendByCategory.length > 0 && (
            <ul className="receipt__list" aria-label="Cost by category">{r.spendByCategory.map((c) => <li key={c.label}><span>{c.label}</span><span className="mono">{fmtUsd(c.usd)}</span></li>)}</ul>
          )}
        </div>
        <div className="receipt__cell"><Eyebrow>Removed</Eyebrow>
          <ul className="receipt__list" aria-label="Removed">
            <li><span><Icon name="check" size={13} /> {n(r.removed.instances, 'instance')} terminated</span></li>
            <li><span><Icon name="check" size={13} /> {n(r.removed.agentIdentities, 'agent identity', 'agent identities')} revoked</span></li>
            <li><span><Icon name="check" size={13} /> {n(r.removed.secrets, 'secret')} deleted</span></li>
            {r.removed.resources.length > 0 && <li><span><Icon name="check" size={13} /> {n(r.removed.resources.length, 'sandbox resource')} destroyed</span></li>}
          </ul>
        </div>
        {!compact && (
          <div className="receipt__cell"><Eyebrow>Runs covered</Eyebrow>
            <ul className="receipt__list" aria-label="Runs covered">{r.runs.map((x) => <li key={x.optimizationId}><span className="mono">{x.optimizationId}</span><span className="faint">{x.status} · {fmtUsd(x.spendUsd)}</span></li>)}</ul>
          </div>
        )}
      </div>
      {(r.orphans.length > 0 || r.errors.length > 0) && (
        <ul className="receipt__problems" role="alert">
          {r.orphans.map((o) => <li key={o}>Left behind: {o}</li>)}
          {r.errors.map((e) => <li key={e}>{e}</li>)}
        </ul>
      )}
    </div>
  );
}
