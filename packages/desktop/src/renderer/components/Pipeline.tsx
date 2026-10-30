import { Fragment } from 'react';
import { Card } from '@ratchet/ui';
import type { BenchPipelineVM, PipelineStage } from '../../shared/types';
import { Icon } from './icons';

const WORD: Record<PipelineStage['state'], string> = { done: 'Done', active: 'Running', failed: 'Failed', idle: 'Waiting' };

export function Pipeline({ pipeline }: { pipeline: BenchPipelineVM }) {
  return (
    <Card className="pipeline" data-testid="pipeline">
      <div className="card-head">
        <h2 className="card-title">Trusted evaluator</h2>
        <span className="card-head__note">eval-only credentials · agents can’t read or change it</span>
        <span className="mono faint card-head__aside" style={{ fontSize: 12 }}>
          {pipeline.candidate ? `candidate ${pipeline.candidate.sha}` : 'idle'}
          {pipeline.queued > 0 && ` · ${pipeline.queued} queued`}
        </span>
      </div>
      <ol className="pipeline__stages" style={{ listStyle: 'none', margin: 0, padding: 0 }}>
        {pipeline.stages.map((s, i) => (
          <Fragment key={s.id}>
            {i > 0 && <li aria-hidden="true" className="pipeline__arrow"><Icon name="chevron" size={14} /></li>}
            <li data-state={s.state} className={`stage stage--${s.state}`}>
              <span className="rt-eyebrow">{i + 1} · {s.label}</span>
              <span className="stage__big">{s.id === 'verdict' && s.state === 'failed' ? 'Not verified' : WORD[s.state]}</span>
              <span className="mono stage__detail">{s.detail || '—'}</span>
            </li>
          </Fragment>
        ))}
      </ol>
    </Card>
  );
}
