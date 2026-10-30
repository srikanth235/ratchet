import { Card, Eyebrow, Sparkline } from '@ratchet/ui';
import type { Kpi } from '../../shared/types';

export function KpiCard({ kpi }: { kpi: Kpi }) {
  return (
    <Card className="kpi">
      <Eyebrow>{kpi.label}</Eyebrow>
      <div className="kpi__value">
        <span className="hero-num">{kpi.value}</span>
        {kpi.unit && <span className="kpi__unit">{kpi.unit}</span>}
      </div>
      <div className="kpi__foot">
        <span className="mono kpi__delta">{kpi.delta}</span>
        <Sparkline values={kpi.trend} width={96} height={30} />
      </div>
    </Card>
  );
}
