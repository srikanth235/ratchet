import { Card, Eyebrow, Pill } from '@ratchet/ui';
import { tokens } from '@ratchet/ui';
import type { PlanChangeVM } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
import { useQuery } from '../data';

const SIGIL: Record<PlanChangeVM['action'], { sym: string; cls: string; label: string }> = {
  create: { sym: '+', cls: 'plan--create', label: 'create' },
  update: { sym: '~', cls: 'plan--update', label: 'update' },
  replace: { sym: '±', cls: 'plan--update', label: 'replace' },
  delete: { sym: '−', cls: 'plan--delete', label: 'destroy' },
  'no-op': { sym: '·', cls: 'plan--noop', label: 'no change' },
};

/** Static architecture sketch: the trusted path (brass) from the control plane through the eval identity to the sealed bucket. */
export function SandboxDiagram() {
  const box = (x: number, y: number, w: number, label: string, sub: string, trusted = false) => (
    <g>
      <rect x={x} y={y} width={w} height={56} rx={10} fill={tokens.color.card} stroke={trusted ? tokens.color.brass : tokens.color.hairline} strokeWidth={trusted ? 1.5 : 1} />
      <text x={x + 12} y={y + 24} fill={tokens.color.text} fontSize="12" fontWeight="600" fontFamily={tokens.font.sans}>{label}</text>
      <text x={x + 12} y={y + 42} fill={tokens.color.textTertiary} fontSize="10.5" fontFamily={tokens.font.mono}>{sub}</text>
    </g>
  );
  const arrow = (x1: number, y1: number, x2: number, y2: number, trusted = false) => (
    <line x1={x1} y1={y1} x2={x2} y2={y2} stroke={trusted ? tokens.color.brass : tokens.color.textTertiary} strokeWidth={trusted ? 2 : 1.2} strokeDasharray={trusted ? undefined : '4 4'} markerEnd={`url(#${trusted ? 'arr-brass' : 'arr-grey'})`} />
  );
  return (
    <svg className="rt-chart sandbox-diagram" viewBox="0 0 760 250" role="img" aria-label="Sandbox architecture: control plane, eval identity and sealed bucket on the trusted path; agent runners confined to the agent boundary">
      <defs>
        <marker id="arr-brass" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0L8 4L0 8z" fill={tokens.color.brass} /></marker>
        <marker id="arr-grey" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0L8 4L0 8z" fill={tokens.color.textTertiary} /></marker>
      </defs>
      <rect x="1" y="1" width="758" height="248" rx="14" fill="none" stroke={tokens.color.hairline} />
      {box(24, 28, 170, 'Ratchet app', 'control plane · your Mac', true)}
      {box(300, 28, 190, 'Eval identity', 'bench + confirm only', true)}
      {box(580, 28, 156, 'Sealed bucket', 'hidden traces', true)}
      {arrow(194, 56, 300, 56, true)}
      {arrow(490, 56, 580, 56, true)}
      <text x="128" y="124" fill={tokens.color.brass} fontSize="10" letterSpacing="1.5" fontFamily={tokens.font.sans} fontWeight="600">TRUSTED PATH</text>
      <line x1="24" y1="140" x2="736" y2="140" stroke={tokens.color.hairline} />
      <text x="128" y="162" fill={tokens.color.textTertiary} fontSize="10" letterSpacing="1.5" fontFamily={tokens.font.sans} fontWeight="600">AGENT BOUNDARY</text>
      {box(24, 178, 170, 'Agent runners', 'EC2 · per-agent role', false)}
      {box(300, 178, 190, 'Experiment instances', 'tagged · lifetime-capped', false)}
      {box(580, 178, 156, 'Artifacts bucket', 'logs · bench output', false)}
      {arrow(194, 206, 300, 206)}
      {arrow(490, 206, 580, 206)}
      {arrow(109, 84, 109, 178)}
    </svg>
  );
}

export function Sandbox() {
  const { data, error } = useQuery(() => window.ratchet.sandboxInfo(), [], { intervalMs: 15_000 });
  if (error && !data) return <ErrorNote error={error} />;
  if (!data) return <p className="muted">Loading…</p>;
  const { plan, outputs, account, limits } = data;

  return (
    <>
      <PageHeader
        title="AWS sandbox"
        sub="Agents run inside a boundary; benchmarking and hidden traces stay on the trusted path."
        actions={account.simulated ? <Pill tone="neutral">Simulated sandbox</Pill> : <Pill tone="brass" dot>Live account</Pill>}
      />

      <Card><Eyebrow>Architecture</Eyebrow><SandboxDiagram /></Card>

      <section className="grid-two">
        <Card>
          <div className="row-between"><Eyebrow>OpenTofu plan</Eyebrow><span className="mono muted">{plan.summary}</span></div>
          <ul className="plan" aria-label="Plan changes">
            {plan.changes.map((c) => {
              const g = SIGIL[c.action];
              return (
                <li key={c.address} className={g.cls}>
                  <span className="plan__sym mono" aria-hidden="true">{g.sym}</span>
                  <span className="mono plan__addr">{c.address}</span>
                  <span className="plan__type muted">{c.type}</span>
                  <span className="plan__act">{g.label}</span>
                </li>
              );
            })}
          </ul>
        </Card>

        <div className="stack">
          <Card>
            <Eyebrow>Account</Eyebrow>
            <dl className="kv">
              <dt>Sandbox</dt><dd className="mono">{account.sandboxId}</dd>
              <dt>Account</dt><dd className="mono">{account.accountId ?? '—'}</dd>
              <dt>Region</dt><dd className="mono">{account.region}</dd>
              <dt>AWS profile</dt><dd className="mono">{account.profile}</dd>
            </dl>
          </Card>
          <Card>
            <Eyebrow>Agent limits</Eyebrow>
            <dl className="kv">
              <dt>Instance types</dt><dd className="mono">{limits.allowedInstanceTypes.join(', ')}</dd>
              <dt>Max instances / agent</dt><dd className="mono">{limits.maxInstancesPerAgent}</dd>
              <dt>Max instance lifetime</dt><dd className="mono">{limits.maxInstanceLifetimeHours} h</dd>
            </dl>
          </Card>
        </div>
      </section>

      <Card>
        <Eyebrow>Contract outputs</Eyebrow>
        {outputs ? (
          <dl className="kv kv--wide">
            <dt>artifactsBucket</dt><dd className="mono">{outputs.artifactsBucket}</dd>
            <dt>sealedBucket</dt><dd className="mono">{outputs.sealedBucket}</dd>
            <dt>agentBoundaryRef</dt><dd className="mono">{outputs.agentBoundaryRef}</dd>
            <dt>evalIdentityRef</dt><dd className="mono">{outputs.evalIdentityRef}</dd>
            <dt>agentRolePath</dt><dd className="mono">{outputs.agentRolePath}</dd>
            <dt>network.subnetId</dt><dd className="mono">{outputs.network.subnetId}</dd>
            <dt>network.agentRunnerSecurityGroupId</dt><dd className="mono">{outputs.network.agentRunnerSecurityGroupId}</dd>
            <dt>network.benchSecurityGroupId</dt><dd className="mono">{outputs.network.benchSecurityGroupId}</dd>
          </dl>
        ) : (
          <p className="muted">The sandbox has not been applied yet.</p>
        )}
      </Card>
    </>
  );
}
