import { Card, Eyebrow, Sparkline } from '@ratchet/ui';
import type { Kpi } from '../../shared/types';

/** `+8 this week` → badge `+8`, foot `this week`. Deltas that do not start with a sign stay whole in the foot. */
export function splitDelta(delta: string): { badge: string | null; foot: string } {
  const m = /^([+−-][\d.,]+(?:%| pts)?)\s*(.*)$/.exec(delta.trim());
  return m ? { badge: m[1]!, foot: m[2] ?? '' } : { badge: null, foot: delta };
}

export function KpiCard({ kpi }: { kpi: Kpi }) {
  const { badge, foot } = splitDelta(kpi.delta);
  return (
    <Card className="kpi" data-testid="kpi">
      <div className="kpi__head">
        <Eyebrow>{kpi.label}</Eyebrow>
        {badge && <span className="kpi__badge mono">{badge}</span>}
      </div>
      <div className="kpi__row">
        <span className="kpi__value">
          <span className="hero-num">{kpi.value}</span>
          {kpi.unit && <span className="kpi__unit">{kpi.unit}</span>}
        </span>
        <Sparkline values={kpi.trend} width={112} height={34} />
      </div>
      {foot && <span className="kpi__foot">{foot}</span>}
    </Card>
  );
}
