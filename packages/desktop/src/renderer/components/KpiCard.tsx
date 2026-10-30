import { Card, Eyebrow, Sparkline } from '@ratchet/ui';
import type { Kpi } from '../../shared/types';

/** One KPI: value for the selected range, change chip vs the previous equal-length range (when there is one). */
export function KpiCard({ kpi, sparkWidth = 112 }: { kpi: Kpi; sparkWidth?: number }) {
  return (
    <Card className="kpi" data-testid="kpi" data-kpi={kpi.id}>
      <div className="kpi__head">
        <Eyebrow>{kpi.label}</Eyebrow>
        {kpi.change && (
          <span className={`kpi__badge kpi__badge--${kpi.change.tone} mono`} title="vs the previous period of the same length">{kpi.change.text}</span>
        )}
      </div>
      <div className="kpi__row">
        <span className="kpi__value">
          <span className="hero-num">{kpi.value}</span>
          {kpi.unit && <span className="kpi__unit">{kpi.unit}</span>}
        </span>
        {kpi.trend.length > 1 && <Sparkline values={kpi.trend} width={sparkWidth} height={34} />}
      </div>
      {kpi.foot && <span className="kpi__foot">{kpi.foot}</span>}
    </Card>
  );
}
