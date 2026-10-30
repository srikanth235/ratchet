import { useState, type ReactNode } from 'react';
import { Button, Card, Pill } from '@ratchet/ui';
import type { DemoId, TemplateVM } from '../../shared/types';
import { Empty, ErrorNote } from '../components/common';
import { UseTemplatePanel } from '../components/UseTemplate';
import { errMsg, reportStartError, useNav, useQuery } from '../data';
import { demoTitle } from '../format';
import { DEFAULT_BUDGET_USD, DEFAULT_TIME_SECONDS } from './NewOptimization';

const CATEGORY: Record<TemplateVM['category'], string> = { perf: 'Performance', forecasting: 'Forecasting', llm: 'LLM' };

function Spec({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="spec">
      <span className="spec__label">{label}</span>
      <span className="spec__value">{children}</span>
    </div>
  );
}

/** Start a demo template in simulation: scripted agents, mock cloud, no setup. */
export function useRunDemo() {
  const { open } = useNav();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async (t: TemplateVM): Promise<void> => {
    setBusy(t.id);
    setError(null);
    try {
      const id = await window.ratchet.createOptimization({
        demo: t.id as DemoId,
        repoUrl: '',
        baseRef: 'main',
        agents: ['claude-code', 'codex'],
        agentsWhere: 'local',
        budgetUsd: DEFAULT_BUDGET_USD,
        timeLimitSeconds: DEFAULT_TIME_SECONDS,
        plateauEvals: 25,
      });
      window.ratchet.startOptimization(id).catch((e: unknown) => reportStartError(id, e));
      open(id, 'agents');
    } catch (e) {
      setError(errMsg(e));
      setBusy(null);
    }
  };
  return { busy, error, run };
}

function TemplateCard({ t, onUse, active }: { t: TemplateVM; onUse(): void; active: boolean }) {
  return (
    <Card className="template-card" data-testid="template-card" data-template={t.id}>
      <div className="template-card__head">
        <span className="template-card__title">{t.title}</span>
        <Pill>{CATEGORY[t.category]}</Pill>
      </div>
      <p className="muted template-card__desc">Use it to {t.description}.</p>
      <div className="specs">
        <Spec label="Measures">{t.measures}</Spec>
        <Spec label="Holdout">{t.holdout ?? 'none'}</Spec>
        <Spec label="Machine"><span className="mono">{t.machine}</span></Spec>
        <Spec label="Objective"><span className="mono">{t.objective.metric}</span> {t.objective.direction === 'minimize' ? '↓' : '↑'}{t.objective.unit ? <span className="faint"> {t.objective.unit}</span> : null}</Spec>
      </div>
      <div className="template-card__foot">
        <Button variant={active ? 'subtle' : 'ghost'} aria-expanded={active} onClick={onUse} aria-label={`Use template ${t.title}`}>Use template</Button>
      </div>
    </Card>
  );
}

/** The evaluator template gallery (Benchmarks > Templates): "Use template" opens a pull request; "Try a demo" runs in simulation. */
export function TemplateGallery() {
  const q = useQuery(() => window.ratchet.listTemplates(), []);
  const [using, setUsing] = useState<string | null>(null);
  const demo = useRunDemo();
  if (q.error && !q.data) return <ErrorNote error={q.error} />;
  if (!q.data) return <p className="muted">Loading…</p>;
  const templates = q.data.filter((t) => t.kind === 'template');
  const demos = q.data.filter((t) => t.kind === 'demo');
  const picked = templates.find((t) => t.id === using) ?? null;

  return (
    <>
      <section className="gallery" aria-label="Evaluator templates">
        <div className="card-head">
          <h2 className="card-title">Evaluator templates</h2>
          <span className="card-head__note">A template adds the evaluator files, a ratchet.yaml and an agent brief to your repository through a pull request.</span>
        </div>
        <div className="template-grid">
          {templates.map((t) => <TemplateCard key={t.id} t={t} active={using === t.id} onUse={() => setUsing(using === t.id ? null : t.id)} />)}
        </div>
        {picked && <UseTemplatePanel key={picked.id} template={picked} onClose={() => setUsing(null)} />}
      </section>

      <section className="gallery" aria-label="Try a demo" data-testid="try-demo">
        <div className="card-head">
          <h2 className="card-title">Try a demo</h2>
          <span className="card-head__note">Runs in simulation with scripted agents and a mock cloud: no setup, nothing billed.</span>
        </div>
        <ErrorNote error={demo.error} />
        <div className="template-grid">
          {demos.map((t) => (
            <Card key={t.id} className="template-card" data-testid="demo-card" data-template={t.id}>
              <div className="template-card__head">
                <span className="template-card__title">{demoTitle(t.title)}</span>
                <Pill tone="brass">Demo</Pill>
              </div>
              <p className="muted template-card__desc">{t.description}</p>
              <div className="specs">
                <Spec label="Measures">{t.measures}</Spec>
                <Spec label="Objective"><span className="mono">{t.objective.metric}</span> {t.objective.direction === 'minimize' ? '↓' : '↑'}</Spec>
              </div>
              <div className="template-card__foot">
                <Button disabled={demo.busy !== null} onClick={() => void demo.run(t)} aria-label={`Run demo ${demoTitle(t.title)}`}>{demo.busy === t.id ? 'Starting…' : 'Run demo'}</Button>
              </div>
            </Card>
          ))}
          {demos.length === 0 && <Empty title="No demos">No demo templates are available in this build.</Empty>}
        </div>
      </section>
    </>
  );
}
