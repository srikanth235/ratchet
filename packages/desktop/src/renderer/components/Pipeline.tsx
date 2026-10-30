import { Card, Eyebrow, Pill } from '@ratchet/ui';
import type { BenchPipelineVM, PipelineStage } from '../../shared/types';

const TONE: Record<PipelineStage['state'], 'brass' | 'mint' | 'rose' | 'neutral'> = {
  done: 'mint', active: 'brass', failed: 'rose', idle: 'neutral',
};

export function Pipeline({ pipeline }: { pipeline: BenchPipelineVM }) {
  return (
    <Card className="pipeline">
      <div className="row-between">
        <Eyebrow>Bench runner</Eyebrow>
        <span className="mono muted">
          {pipeline.candidate ? `candidate ${pipeline.candidate.sha}` : 'idle'}
          {pipeline.queued > 0 && ` · ${pipeline.queued} queued`}
        </span>
      </div>
      <ol className="pipeline__stages">
        {pipeline.stages.map((s) => (
          <li key={s.id} data-state={s.state} className={`stage stage--${s.state}`}>
            <span className="stage__label">{s.label}</span>
            <Pill tone={TONE[s.state]} dot live={s.state === 'active'}>{s.id === 'verdict' && s.state === 'failed' ? 'not verified' : s.state}</Pill>
            <span className="mono muted stage__detail">{s.detail || '—'}</span>
          </li>
        ))}
      </ol>
    </Card>
  );
}
