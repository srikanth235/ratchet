import { useEffect, useState } from 'react';
import { Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import type { PlanChangeVM, SandboxInfo, SandboxStatusVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { errMsg, useNav, useQuery } from '../data';

const SIGIL: Record<PlanChangeVM['action'], { cls: string; label: string }> = {
  create: { cls: 'plan--create', label: '+ create' },
  update: { cls: 'plan--update', label: '~ update' },
  replace: { cls: 'plan--update', label: '± replace' },
  delete: { cls: 'plan--delete', label: '− destroy' },
  'no-op': { cls: 'plan--noop', label: 'no change' },
};

/** `aws_s3_bucket_policy.x` → S3, `aws_iam_role.x` → IAM, `aws_security_group.x` → EC2. */
export function resourceKind(type: string): { label: string; cls: string } {
  if (/^aws_s3/.test(type)) return { label: 'S3', cls: 'plan__kind--s3' };
  if (/^aws_iam/.test(type)) return { label: 'IAM', cls: 'plan__kind--iam' };
  if (/^aws_(security_group|instance|launch|vpc|subnet|ec2)/.test(type)) return { label: 'EC2', cls: 'plan__kind--ec2' };
  return { label: (type.replace(/^aws_/, '').split('_')[0] ?? '·').slice(0, 3).toUpperCase(), cls: '' };
}

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
      <svg viewBox="0 0 1016 340" role="img" aria-label="Sandbox architecture: control plane, evaluator runner and sealed traces on the trusted path; agent runners confined to the agent boundary" className="rt-chart sandbox-diagram">
        <defs>
          <marker id="sb-grey" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0L8 4L0 8z" fill={grey} /></marker>
          <marker id="sb-brass" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0L8 4L0 8z" fill="#D9B26F" /></marker>
        </defs>
        <rect x="264" y="168" width="744" height="156" rx="14" fill="rgba(217,178,111,.04)" stroke="rgba(217,178,111,.28)" strokeDasharray="5 5" />
        <path d="M112 112 V232 H288" fill="none" stroke={grey} strokeWidth="1.5" markerEnd="url(#sb-grey)" />
        <path d="M376 196 V76 H552" fill="none" stroke={grey} strokeWidth="1.5" markerEnd="url(#sb-grey)" />
        <path d="M640 112 V196" fill="none" stroke="#D9B26F" strokeWidth="2" markerEnd="url(#sb-brass)" />
        <path d="M816 232 H728" fill="none" stroke="#D9B26F" strokeWidth="2" markerEnd="url(#sb-brass)" />
        <Node x={24} y={40} title="Ratchet.app" sub="your Mac · control" icon="M0 0h14v9H0zM4 13h6" />
        <Node x={552} y={40} title="GitHub" sub="one branch per agent" icon="M3 0a2 2 0 1 0 0 .01M3 12a2 2 0 1 0 0 .01M11 4a2 2 0 1 0 0 .01M3 2v8M11 6c0 4-8 2-8 4" />
        <Node x={288} y={196} title="Agent runners" sub="Claude Code · Codex" icon="M0 0h14v10H0zM3 3l2 2-2 2M7 8h3" />
        <Node x={552} y={196} title="Evaluator runner" sub="eval-only role" trusted icon="M0 10a7 7 0 0 1 14 0M7 10l3-4" />
        <Node x={816} y={196} title="Sealed traces" sub="S3 · deny agents" trusted icon="M2 6h10v7H2zM4 6V4a3 3 0 0 1 6 0v2" />
        <text x="124" y="176" fill="#A7B1C2" {...label}>launch · SSM</text>
        <text x="448" y="62" fill="#A7B1C2" {...label}>push</text>
        <text x="392" y="140" fill="#A7B1C2" {...label}>headless agent</text>
        <text x="656" y="158" fill="#EBCB8E" {...label}>re-evaluate</text>
        <text x="756" y="216" fill="#EBCB8E" {...label}>read</text>
        <text x="284" y="310" fill="#C9A86A" fontSize="10.5" letterSpacing="1.5" fontWeight="600" fontFamily="Instrument Sans, sans-serif">YOUR AWS ACCOUNT · {region.toUpperCase()} · {sandboxId.toUpperCase()}</text>
      </svg>
    </div>
  );
}

const mask = (id: string | null) => (id && id.length >= 8 ? `${id.slice(0, 4)} ···· ${id.slice(-4)}` : (id ?? '—'));

type Pending = 'plan' | 'apply' | 'destroy';

const BUSY: SandboxStatusVM[] = ['applying', 'destroying'];

function StatusPill({ info }: { info: SandboxInfo }) {
  const toAdd = info.plan?.changes.filter((c) => c.action === 'create').length ?? 0;
  switch (info.status) {
    case 'applied':
      return <Pill tone="mint" dot large>Applied</Pill>;
    case 'applying':
      return <Pill tone="brass" dot live large>Applying…</Pill>;
    case 'destroying':
      return <Pill tone="brass" dot live large>Destroying…</Pill>;
    case 'destroyed':
      return <Pill tone="neutral" dot large>Destroyed</Pill>;
    case 'failed':
      return <Pill tone="rose" dot large>Failed</Pill>;
    default:
      return <Pill tone="brass" dot large>{info.plan ? `Plan ready · ${toAdd} to add` : 'Not planned yet'}</Pill>;
  }
}

/** The confirmation shown before anything is created or destroyed. */
function ConfirmCard({ kind, info, busy, onConfirm, onCancel }: { kind: 'apply' | 'destroy'; info: SandboxInfo; busy: boolean; onConfirm(): void; onCancel(): void }) {
  const { plan, account } = info;
  const n = (a: PlanChangeVM['action']) => plan?.changes.filter((c) => c.action === a).length ?? 0;
  if (kind === 'apply') {
    return (
      <Card className="confirm-card">
        <div role="group" aria-label="Confirm apply" className="confirm-card">
          <h2 className="confirm-card__title">Apply this plan{account.simulated ? ' (simulated)' : ''}?</h2>
          <p className="confirm-card__body">
            {account.simulated
              ? 'Simulation: the mock sandbox records its outputs in the ledger and nothing is created in AWS.'
              : `OpenTofu will create real resources in AWS${account.accountId ? ` account ${mask(account.accountId)}` : ''} (region ${account.region}, profile ${account.profile}). Idle cost is roughly $29 a month for the network endpoints; destroy the sandbox when you are done.`}
          </p>
          <div className="confirm-card__facts mono">
            <span>{plan?.summary ?? 'No plan'}</span>
            <span>+{n('create')} ~{n('update') + n('replace')} −{n('delete')}</span>
          </div>
          <div className="confirm-card__actions">
            <Button disabled={busy} onClick={onConfirm}>Confirm apply</Button>
            <Button variant="ghost" disabled={busy} onClick={onCancel}>Cancel</Button>
          </div>
        </div>
      </Card>
    );
  }
  return (
    <Card className="confirm-card confirm-card--danger">
      <div role="group" aria-label="Confirm destroy" className="confirm-card">
        <h2 className="confirm-card__title">Destroy sandbox {account.sandboxId}?</h2>
        <p className="confirm-card__body">
          {account.simulated
            ? 'Simulation: the mock sandbox and its recorded outputs are removed.'
            : 'OpenTofu will delete the buckets (including the sealed holdout traces and all artifacts), the network and the evaluator identities. This cannot be undone. It is refused while an optimization is running or any instance or agent identity is left.'}
        </p>
        <div className="confirm-card__actions">
          <Button variant="danger" disabled={busy} onClick={onConfirm}>Confirm destroy</Button>
          <Button variant="ghost" disabled={busy} onClick={onCancel}>Cancel</Button>
        </div>
      </div>
    </Card>
  );
}

export function Sandbox() {
  const [pollMs, setPollMs] = useState(15_000);
  const { data, error, refresh, loading } = useQuery(() => window.ratchet.sandboxInfo(), [], { intervalMs: pollMs });
  const { go } = useNav();
  const [local, setLocal] = useState<SandboxInfo | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [confirm, setConfirm] = useState<'apply' | 'destroy' | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  // A fresh read replaces what an action returned; poll quickly while OpenTofu is working.
  useEffect(() => {
    setLocal(null);
    setPollMs(data && BUSY.includes(data.status) ? 2_000 : 15_000);
  }, [data]);

  const info = local ?? data;
  if (error && !info) return <ErrorNote error={error} />;
  if (!info) return <p className="muted">Loading…</p>;

  const { plan, outputs, account, limits } = info;
  const count = (a: PlanChangeVM['action']) => plan?.changes.filter((c) => c.action === a).length ?? 0;
  const toAdd = count('create');
  const busy = pending !== null || BUSY.includes(info.status);
  const changesPending = plan ? plan.changes.some((c) => c.action !== 'no-op') : false;
  const needsPlan = !plan && !account.simulated;
  const canApplyNow = info.status === 'applied' ? changesPending : !BUSY.includes(info.status);
  const outRows: [string, string | null][] = [
    ['artifacts_bucket', outputs?.artifactsBucket ?? null],
    ['sealed_bucket', outputs?.sealedBucket ?? null],
    ['agent_boundary_ref', outputs?.agentBoundaryRef ?? null],
    ['eval_identity_ref', outputs?.evalIdentityRef ?? null],
    ['network_ref', outputs?.network.subnetId ?? null],
  ];

  const run = async (what: Pending, fn: () => Promise<SandboxInfo>) => {
    setPending(what);
    setActionError(null);
    try {
      setLocal(await fn());
    } catch (e) {
      setActionError(errMsg(e));
      refresh();
    } finally {
      setConfirm(null);
      setPending(null);
    }
  };

  return (
    <>
      <PageHeader
        eyebrow="Environments"
        title="AWS sandbox"
        badge={<><StatusPill info={info} />{account.simulated ? <Pill tone="neutral" large>Simulated sandbox</Pill> : <Pill tone="brass" large>Live account</Pill>}</>}
        sub="A fenced area in a dedicated account. Agents run headless on runners inside it; the trusted evaluator runner sits outside their reach."
        actions={
          <>
            <Button variant="ghost" disabled={loading || busy || info.setupProblems.length > 0} onClick={() => void run('plan', () => window.ratchet.planSandbox())}>
              {needsPlan ? 'Plan' : 'Re-plan'}
            </Button>
            {canApplyNow && (
              <Button disabled={busy || !info.canApply || !plan} title={!plan ? 'Review the plan first' : undefined} onClick={() => setConfirm('apply')}>
                {info.status === 'applied' ? 'Apply changes' : 'Apply sandbox'}
              </Button>
            )}
            {(info.status === 'applied' || info.status === 'failed') && (
              <Button variant="danger" disabled={busy || !info.canDestroy} title={info.destroyBlockedReason ?? undefined} onClick={() => setConfirm('destroy')}>
                Destroy sandbox
              </Button>
            )}
          </>
        }
      />
      <ErrorNote error={actionError ?? (info.status === 'failed' ? (info.lastError ?? 'The last apply or destroy failed; run it again to converge.') : null)} />
      {info.setupProblems.length > 0 && (
        <div className="setup-note" role="status">
          Live setup is incomplete:
          <ul>{info.setupProblems.map((p) => <li key={p}>{p}</li>)}</ul>
          <Button variant="subtle" onClick={() => go('settings')}>Open Settings</Button>
        </div>
      )}
      {info.destroyBlockedReason && (info.status === 'applied' || info.status === 'failed') && <p className="muted" role="status">Destroy unavailable: {info.destroyBlockedReason}</p>}
      {confirm && (
        <ConfirmCard
          kind={confirm}
          info={info}
          busy={busy}
          onCancel={() => setConfirm(null)}
          onConfirm={() => void run(confirm, () => (confirm === 'apply' ? window.ratchet.applySandbox() : window.ratchet.destroySandbox()))}
        />
      )}

      <Card className="diagram-card">
        <div className="card-head">
          <h2 className="card-title">How it fits together</h2>
          <span className="card-head__note">Brass lines are the trusted path agents can’t touch.</span>
        </div>
        <SandboxDiagram region={account.region} sandboxId={account.sandboxId} />
      </Card>

      <section className="sandbox-split">
        <Card className="plan-card">
          <div className="plan-card__head">
            <h2 className="card-title">OpenTofu plan</h2>
            <span className="mono faint" style={{ fontSize: 12 }}>{plan?.summary ?? 'No plan yet'}</span>
            <span className="plan__counts mono" aria-label="Plan totals">
              <span style={{ color: 'var(--rt-color-mint)' }}>+{toAdd}</span><span>~{count('update') + count('replace')}</span><span>−{count('delete')}</span>
            </span>
          </div>
          {plan ? (
            <ul className="plan" aria-label="Plan changes">
              {plan.changes.map((c) => {
                const g = SIGIL[c.action];
                const k = resourceKind(c.type);
                return (
                  <li key={c.address} className={g.cls}>
                    <span className={`plan__kind mono ${k.cls}`} aria-hidden="true">{k.label}</span>
                    <span className="plan__main">
                      <span className="mono plan__addr">{c.address}</span>
                      <span className="plan__type">{c.type}</span>
                    </span>
                    <span className="plan__act mono">{g.label}</span>
                  </li>
                );
              })}
            </ul>
          ) : (
            <div className="plan-empty">
              <p className="muted">A real plan runs <span className="mono">tofu plan</span> against your account, so it only runs when you ask. Review it here before anything is applied.</p>
              <Button variant="subtle" disabled={busy || info.setupProblems.length > 0} onClick={() => void run('plan', () => window.ratchet.planSandbox())}>Run plan</Button>
            </div>
          )}
        </Card>

        <div className="side-col">
          <Card className="side-card">
            <Eyebrow>Account</Eyebrow>
            <div className="kv-row"><span>Profile</span><span className="mono">{account.profile}</span></div>
            <div className="kv-row"><span>Account</span><span className="mono">{mask(account.accountId)}</span></div>
            <div className="kv-row"><span>Region</span><span className="mono">{account.region}</span></div>
            <div className="kv-row"><span>Sandbox</span><span className="mono">{account.sandboxId}</span></div>
            <div className="kv-row"><span>Optimizations</span><span className="mono">{info.optimizations.length}</span></div>
          </Card>
          <Card className="side-card side-card--gap">
            <Eyebrow>Agent limits</Eyebrow>
            <div className="type-chips">
              <span className="sr-only">Allowed instance types</span>
              {limits.allowedInstanceTypes.map((t) => <span key={t} className="chip-tag mono type-chip">{t}</span>)}
            </div>
            <div className="limits">
              <div className="limit-tile"><div className="limit-tile__label">Max instances / agent</div><div className="serif">{limits.maxInstancesPerAgent}</div></div>
              <div className="limit-tile"><div className="limit-tile__label">Max instance lifetime</div><div className="serif">{limits.maxInstanceLifetimeHours} h</div></div>
            </div>
          </Card>
          <Card className="side-card outputs">
            <Eyebrow>Outputs · same on every cloud</Eyebrow>
            {outRows.map(([k, v]) => (
              <div key={k} className="kv-row mono"><span style={{ color: 'var(--rt-color-text)' }}>{k}</span><span>{v ?? 'after apply'}</span></div>
            ))}
          </Card>
        </div>
      </section>
    </>
  );
}
