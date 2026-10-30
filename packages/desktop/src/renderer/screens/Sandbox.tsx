import { Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import type { PlanChangeVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { useQuery } from '../data';

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

/** Architecture sketch: the trusted path (brass) from the control plane through the bench runner to the sealed traces. */
export function SandboxDiagram({ region, sandboxId }: { region: string; sandboxId: string }) {
  const grey = '#8592A6';
  const label = { fontSize: 13, fontFamily: 'Instrument Sans, sans-serif' } as const;
  return (
    <div className="diagram rt-dots" style={{ padding: 0 }}>
      <svg viewBox="0 0 1016 340" role="img" aria-label="Sandbox architecture: control plane, bench runner and sealed traces on the trusted path; agent runners confined to the agent boundary" className="rt-chart sandbox-diagram">
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
        <Node x={552} y={196} title="Bench runner" sub="eval-only role" trusted icon="M0 10a7 7 0 0 1 14 0M7 10l3-4" />
        <Node x={816} y={196} title="Sealed traces" sub="S3 · deny agents" trusted icon="M2 6h10v7H2zM4 6V4a3 3 0 0 1 6 0v2" />
        <text x="124" y="176" fill="#A7B1C2" {...label}>launch · SSM</text>
        <text x="448" y="62" fill="#A7B1C2" {...label}>push</text>
        <text x="392" y="140" fill="#A7B1C2" {...label}>headless agent</text>
        <text x="656" y="158" fill="#EBCB8E" {...label}>re-bench</text>
        <text x="756" y="216" fill="#EBCB8E" {...label}>read</text>
        <text x="284" y="310" fill="#C9A86A" fontSize="10.5" letterSpacing="1.5" fontWeight="600" fontFamily="Instrument Sans, sans-serif">YOUR AWS ACCOUNT · {region.toUpperCase()} · {sandboxId.toUpperCase()}</text>
      </svg>
    </div>
  );
}

const mask = (id: string | null) => (id && id.length >= 8 ? `${id.slice(0, 4)} ···· ${id.slice(-4)}` : (id ?? '—'));

export function Sandbox() {
  const { data, error, refresh, loading } = useQuery(() => window.ratchet.sandboxInfo(), [], { intervalMs: 15_000 });
  if (error && !data) return <ErrorNote error={error} />;
  if (!data) return <p className="muted">Loading…</p>;
  const { plan, outputs, account, limits } = data;
  const count = (a: PlanChangeVM['action']) => plan.changes.filter((c) => c.action === a).length;
  const toAdd = count('create');
  const status = outputs
    ? <Pill tone="mint" dot large>Applied</Pill>
    : <Pill tone="brass" dot large>Plan ready · {toAdd} to add</Pill>;
  const outRows: [string, string | null][] = [
    ['artifacts_bucket', outputs?.artifactsBucket ?? null],
    ['sealed_bucket', outputs?.sealedBucket ?? null],
    ['agent_boundary_ref', outputs?.agentBoundaryRef ?? null],
    ['eval_identity_ref', outputs?.evalIdentityRef ?? null],
    ['network_ref', outputs?.network.subnetId ?? null],
  ];

  return (
    <>
      <PageHeader
        eyebrow="Environments"
        title="AWS sandbox"
        badge={<>{status}{account.simulated ? <Pill tone="neutral" large>Simulated sandbox</Pill> : <Pill tone="brass" large>Live account</Pill>}</>}
        sub="A fenced area in a dedicated account. Agents run headless on runners inside it; the trusted bench runner sits outside their reach."
        actions={<Button variant="ghost" disabled={loading} onClick={refresh}>Re-plan</Button>}
      />

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
            <span className="mono faint" style={{ fontSize: 12 }}>{plan.summary}</span>
            <span className="plan__counts mono" aria-label="Plan totals">
              <span style={{ color: 'var(--rt-color-mint)' }}>+{toAdd}</span><span>~{count('update') + count('replace')}</span><span>−{count('delete')}</span>
            </span>
          </div>
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
        </Card>

        <div className="side-col">
          <Card className="side-card">
            <Eyebrow>Account</Eyebrow>
            <div className="kv-row"><span>Profile</span><span className="mono">{account.profile}</span></div>
            <div className="kv-row"><span>Account</span><span className="mono">{mask(account.accountId)}</span></div>
            <div className="kv-row"><span>Region</span><span className="mono">{account.region}</span></div>
            <div className="kv-row"><span>Sandbox</span><span className="mono">{account.sandboxId}</span></div>
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
