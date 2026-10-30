import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import type { AgentKind, AgentsWhere, DemoId, HarnessVM, PreflightItem, RepoInspectVM, RepoVM, TemplateVM } from '../../shared/types';
import { Crumbs, ErrorNote, PageHeader } from '../components/common';
import { Icon } from '../components/icons';
import { RepoPicker } from '../components/RepoPicker';
import { PrResult, useProposeTemplate } from '../components/UseTemplate';
import { errMsg, reportStartError, useNav, useQuery } from '../data';
import { demoTitle } from '../format';

const AGENT_CHOICES: { kind: AgentKind; label: string; hint: string }[] = [
  { kind: 'claude-code', label: 'Claude Code', hint: 'Anthropic · claude -p headless' },
  { kind: 'codex', label: 'Codex', hint: 'OpenAI · codex exec headless' },
];

const TIME_LIMITS = [
  { label: '1 hour', seconds: 3600 },
  { label: '2 hours', seconds: 7200 },
  { label: '4 hours', seconds: 14_400 },
  { label: '8 hours', seconds: 28_800 },
  { label: '12 hours', seconds: 43_200 },
  { label: '24 hours', seconds: 86_400 },
];

/** Defaults of step 3. */
export const DEFAULT_BUDGET_USD = 50;
export const DEFAULT_TIME_SECONDS = 7200;
const PLATEAU_EVALS = 25;

type Source = { kind: 'demo'; template: TemplateVM } | { kind: 'repo'; inspect: RepoInspectVM };

/** Numbered step header: serif index badge, title, subtitle and an optional right-hand pill. */
function StepHead({ n, title, sub, tone, aside }: { n: number; title: string; sub: string; tone: 'ready' | 'focus' | 'idle'; aside?: ReactNode }) {
  return (
    <div className="step__head">
      <span className={`step__num${tone === 'ready' ? ' step__num--ready' : tone === 'focus' ? ' step__num--focus' : ''}`} aria-hidden="true">{n}</span>
      <div className="step__titles">
        <h2>{title}</h2>
        <span className="step__sub">{sub}</span>
      </div>
      {aside}
    </div>
  );
}

function Stepper({ step, reached, onGo }: { step: 1 | 2 | 3; reached: 1 | 2 | 3; onGo(s: 1 | 2 | 3): void }) {
  const steps = [{ n: 1, label: 'Repo' }, { n: 2, label: 'Review' }, { n: 3, label: 'Budget & time' }] as const;
  return (
    <nav className="stepper" aria-label="Steps">
      {steps.map((s) => (
        <button key={s.n} type="button" className={`stepper__item${s.n === step ? ' stepper__item--on' : ''}`} aria-current={s.n === step ? 'step' : undefined} disabled={s.n > reached} onClick={() => onGo(s.n)}>
          <span className="stepper__n" aria-hidden="true">{s.n}</span>{s.label}
        </button>
      ))}
    </nav>
  );
}

// ------------------------------------------------------------------ step 1

function TemplateOffer({ repo, baseRef, templates, onRecheck }: { repo: string; baseRef: string; templates: TemplateVM[]; onRecheck(): void }) {
  const p = useProposeTemplate();
  return (
    <div className="template-offer" data-testid="template-offer">
      <p className="muted"><span className="mono">{repo}</span> has no <span className="mono">ratchet.yaml</span> on <span className="mono">{baseRef}</span>. Pick a template and Ratchet opens a pull request that adds it. The repository owns the config; changes go through a pull request.</p>
      <ErrorNote error={p.error} />
      {p.result && (
        <>
          <PrResult r={p.result} />
          <div><Button variant="ghost" onClick={onRecheck}>I merged it: check again</Button></div>
        </>
      )}
      <ul className="template-offer__list" aria-label="Templates">
        {templates.map((t) => (
          <li key={t.id} className="template-offer__item">
            <span className="template-offer__main">
              <span className="template-offer__title">{t.title}</span>
              <span className="muted">{t.description}</span>
              <span className="faint mono">{t.objective.metric} · {t.machine}</span>
            </span>
            <Button variant="ghost" disabled={p.busy} onClick={() => void p.run(repo, baseRef, t)} aria-label={`Add ${t.title} via pull request`}>Add via pull request</Button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function RepoStep({ source, onSource, onNext }: { source: Source | null; onSource(s: Source | null): void; onNext(): void }) {
  const [picked, setPicked] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastSource, setLastSource] = useState<{ kind: 'github'; repo: string } | { kind: 'local'; path: string } | null>(null);
  const templates = useQuery(() => window.ratchet.listTemplates(), []);
  const demos = (templates.data ?? []).filter((t) => t.kind === 'demo');

  async function inspect(s: { kind: 'github'; repo: string } | { kind: 'local'; path: string }): Promise<void> {
    setBusy(true);
    setError(null);
    setLastSource(s);
    try {
      onSource({ kind: 'repo', inspect: await window.ratchet.inspectRepo(s) });
    } catch (e) {
      onSource(null);
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  const pickLocal = async () => {
    setError(null);
    try {
      const path = await window.ratchet.pickLocalFolder();
      if (!path) return;
      setPicked(`${path}`);
      await inspect({ kind: 'local', path });
    } catch (e) {
      setError(errMsg(e));
    }
  };

  const ins = source?.kind === 'repo' ? source.inspect : null;
  const usable = source?.kind === 'demo' || (ins !== null && ins.problem === null && ins.config !== null && ins.repo !== null);

  return (
    <Card className="step step--focus">
      <StepHead n={1} title="Repo" sub="The repository declares what to measure" tone={usable ? 'ready' : 'focus'}
        aside={usable ? <Pill tone="mint"><Icon name="check" size={12} /> Ready</Pill> : undefined} />
      <RepoPicker selected={ins?.repo ?? picked} onPick={(r: RepoVM) => { setPicked(r.fullName); void inspect({ kind: 'github', repo: r.fullName }); }} onLocal={() => void pickLocal()} />
      <ErrorNote error={error} />
      {busy && <p className="muted" role="status">Reading ratchet.yaml…</p>}

      {ins && !busy && (
        <div className="repo-found" data-testid="repo-found">
          {ins.problem && <p role="alert" className="error-note">{ins.problem}</p>}
          {!ins.problem && ins.config && (
            <p className="ok-note" role="status">
              <Icon name="check" size={13} />
              <span>Found <span className="mono">ratchet.yaml</span> on <span className="mono">{ins.baseRef}</span> of <span className="mono">{ins.repo}</span>: {ins.config.metric} ({ins.config.direction === 'minimize' ? 'lower' : 'higher'} is better).</span>
            </p>
          )}
          {!ins.problem && ins.configYaml !== null && !ins.config && (
            <div role="alert" className="error-note">
              <p>ratchet.yaml in {ins.repo} does not parse. Fix it through a pull request:</p>
              <ul className="errors">{ins.errors.map((e, i) => <li key={i}>{e.path && <code className="mono">{e.path}</code>} {e.message}</li>)}</ul>
            </div>
          )}
          {!ins.problem && ins.configYaml === null && ins.repo && (
            <TemplateOffer repo={ins.repo} baseRef={ins.baseRef} templates={ins.templates} onRecheck={() => lastSource && void inspect(lastSource)} />
          )}
        </div>
      )}

      <div className="demo-section" data-testid="demos">
        <Eyebrow>Or try a demo</Eyebrow>
        <p className="muted">Runs in simulation with scripted agents and a mock cloud. Nothing to set up, nothing billed.</p>
        <div className="agent-choices" role="group" aria-label="Demos">
          {demos.map((t) => {
            const on = source?.kind === 'demo' && source.template.id === t.id;
            return (
              <button key={t.id} type="button" aria-pressed={on} className={`agent-choice demo-choice${on ? ' agent-choice--on' : ''}`} onClick={() => { setPicked(null); setError(null); onSource({ kind: 'demo', template: t }); }}>
                <span className="agent-choice__text">
                  <span className="agent-choice__name">{demoTitle(t.title)}</span>
                  <span className="agent-choice__hint">{t.description}</span>
                  <span className="agent-choice__hint mono">{t.objective.metric} · {t.objective.direction === 'minimize' ? '↓' : '↑'}</span>
                </span>
                <span className={`agent-choice__check${on ? ' agent-choice__check--on' : ''}`} aria-hidden="true">{on && <Icon name="check" size={12} />}</span>
              </button>
            );
          })}
        </div>
      </div>
      <div className="step__foot"><Button disabled={!usable || busy} onClick={onNext}>Continue to review</Button></div>
    </Card>
  );
}

// ------------------------------------------------------------------ step 2

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="fact">
      <span className="fact__label">{label}</span>
      <span className="fact__value">{children}</span>
    </div>
  );
}

function ReviewStep({ source, agents, setAgents, where, setWhere, harnesses, missing, onBack, onNext }: {
  source: Source; agents: AgentKind[]; setAgents(a: AgentKind[]): void; where: AgentsWhere; setWhere(w: AgentsWhere): void;
  harnesses: HarnessVM[] | null; missing: PreflightItem[]; onBack(): void; onNext(): void;
}) {
  const { go } = useNav();
  const demo = source.kind === 'demo';
  const cfg = source.kind === 'repo' ? source.inspect.config : null;
  const t = source.kind === 'demo' ? source.template : null;
  const blocking = missing.filter((m) => m.state === 'fail');
  const toggle = (k: AgentKind) => setAgents(agents.includes(k) ? agents.filter((a) => a !== k) : [...agents, k]);
  const detected = (k: AgentKind) => harnesses?.find((h) => h.kind === k);

  return (
    <Card className="step step--focus">
      <StepHead n={2} title="Review" sub="Read from the repo; change it with a pull request" tone="focus" />
      <div className="facts" aria-label="What will run">
        {cfg && (
          <>
            <Fact label="Measured">
              <span className="mono">{cfg.metric}</span> {cfg.direction === 'minimize' ? '↓ lower is better' : '↑ higher is better'}
              <span className="fact__sub">{cfg.evaluator.template ? <>evaluator template <span className="mono">{cfg.evaluator.template}</span></> : <>evaluator <span className="mono">{cfg.evaluator.run}</span></>}{cfg.evaluator.holdout ? (cfg.evaluator.template ? ` · hidden holdout (${cfg.evaluator.holdout}) re-checks every win` : ' · hidden holdout re-checks every win') : ''}</span>
            </Fact>
            <Fact label="Toolchain">
              {cfg.environment.image ? <span className="mono">{cfg.environment.image}</span> : <>host</>}
              <span className="fact__sub">{cfg.environment.image ? 'the repo’s container builds the setup, guardrails and evaluator' : 'no container: setup and evaluator run on the bench machine itself'}</span>
            </Fact>
            <Fact label="Machine">
              <span className="mono">{cfg.environment.machine}</span>
              <span className="fact__sub">pinned bench hardware · {cfg.environment.region ?? 'default region'}</span>
            </Fact>
            <Fact label="Calibration">
              Noise is measured during the baseline
              <span className="fact__sub">{cfg.evaluator.repeats !== null ? `${cfg.evaluator.repeats} runs per candidate (set by the repo)` : 'runs per candidate and the win threshold are set from it, then shown in plain words'}</span>
            </Fact>
          </>
        )}
        {t && (
          <>
            <Fact label="Measured"><span className="mono">{t.objective.metric}</span> {t.objective.direction === 'minimize' ? '↓ lower is better' : '↑ higher is better'}<span className="fact__sub">{t.measures}</span></Fact>
            <Fact label="Toolchain">simulation<span className="fact__sub">the demo project is built and evaluated on this machine</span></Fact>
            <Fact label="Machine">mock AWS<span className="fact__sub">no instances are created</span></Fact>
            <Fact label="Calibration">Noise is measured during the baseline<span className="fact__sub">runs per candidate and the win threshold are set from it</span></Fact>
          </>
        )}
      </div>
      {cfg && cfg.protectedEntries.length > 0 && (
        <div className="protected-row" aria-label="Protected paths">Protected from agents{cfg.protectedEntries.map((p) => <span key={p.glob} className="chip-tag mono" title={p.reason} style={{ color: '#C3CBD8', padding: '4px 8px', borderRadius: 6 }}>{p.glob}</span>)}</div>
      )}
      {source.kind === 'repo' && source.inspect.configYaml !== null && (
        <details className="advanced">
          <summary>View ratchet.yaml</summary>
          <pre className="yaml mono" aria-label="ratchet.yaml (read-only)">{source.inspect.configYaml}</pre>
          <p className="muted">Read-only: the repository owns this file. Change it through a pull request.</p>
        </details>
      )}

      <div className="field">
        <span className="field__label">Agents</span>
        <div className="agent-choices" role="group" aria-label="Agents">
          {AGENT_CHOICES.map((a) => {
            const on = demo || agents.includes(a.kind);
            const h = detected(a.kind);
            const hint = demo ? 'scripted demo agent' : h ? (h.installed ? `${h.version ? `v${h.version} · ` : ''}${h.auth === 'signed-out' ? 'signed out' : h.auth === 'api-key' ? 'API key' : 'signed in'}` : 'not installed') : a.hint;
            return (
              <button key={a.kind} type="button" aria-pressed={on} disabled={demo} className={`agent-choice agent-choice--${a.kind}${on ? ' agent-choice--on' : ''}`} onClick={() => toggle(a.kind)}>
                <AgentAvatar agent={a.kind} size={38} />
                <span className="agent-choice__text">
                  <span className="agent-choice__name">{a.label}</span>
                  <span className="agent-choice__hint mono">{hint}</span>
                </span>
                <span className={`agent-choice__check${on ? ' agent-choice__check--on' : ''}`} aria-hidden="true">{on && <Icon name="check" size={12} />}</span>
              </button>
            );
          })}
        </div>
      </div>

      <div className="field">
        <span className="field__label">Where agents run</span>
        <div className="segmented" role="radiogroup" aria-label="Where agents run">
          {([['local', 'This Mac'], ['cloud', 'Cloud (unattended)']] as const).map(([k, label]) => (
            <button key={k} type="button" role="radio" aria-checked={where === k} disabled={demo && k === 'cloud'} className={where === k ? 'segmented__on' : ''} onClick={() => setWhere(k)}>{label}</button>
          ))}
        </div>
        <span className="field__hint">{demo ? 'Demos run in simulation.' : where === 'local' ? 'Uses the CLIs and logins on this machine. The Mac stays awake while a run is live. The evaluator still runs in the cloud sandbox.' : 'Runs on cloud runners with the laptop closed. Needs API keys.'}</span>
      </div>

      <div className="missing" aria-label="What's missing" data-testid="missing">
        {blocking.length > 0 ? (
          <>
            <Eyebrow>What’s missing</Eyebrow>
            <ul className="preflight">
              {blocking.map((p) => (
                <li key={p.id} data-state={p.state}>
                  <span className="preflight__icon preflight__icon--fail" aria-hidden="true"><Icon name="warn" size={14} /></span>
                  <span><span>{p.label}</span><span className="preflight__detail">{p.detail}</span></span>
                </li>
              ))}
            </ul>
            <div><Button variant="subtle" onClick={() => go('settings')}>Open Settings</Button></div>
          </>
        ) : (
          <p className="ok-note" role="status"><Icon name="check" size={13} /><span>Everything this run needs is in place.</span></p>
        )}
      </div>
      <div className="step__foot">
        <Button variant="ghost" onClick={onBack}>Back</Button>
        <Button disabled={blocking.length > 0 || (!demo && agents.length === 0)} onClick={onNext}>Continue to budget</Button>
      </div>
    </Card>
  );
}

// ------------------------------------------------------------------ step 3

function BudgetStep({ budget, setBudget, seconds, setSeconds, submitting, onBack, onStart }: {
  budget: number; setBudget(n: number): void; seconds: number; setSeconds(n: number): void; submitting: boolean; onBack(): void; onStart(): void;
}) {
  return (
    <Card className="step step--focus">
      <StepHead n={3} title="Budget & time" sub="Hard limits for the whole run" tone="focus" />
      <div className="budget">
        <label className="budget__top" htmlFor="budget-range">
          <span>{`Budget · $${budget}`}</span>
          <span className="budget__value" aria-hidden="true">${budget}</span>
        </label>
        <input id="budget-range" type="range" min={5} max={500} step={5} value={budget} aria-label="Budget" style={{ width: '100%', accentColor: '#D9B26F' }} onChange={(e) => setBudget(Number(e.target.value))} />
        <span className="budget__scale mono"><span>$5</span><span>warn at 80% · hard stop at 100%</span><span>$500</span></span>
      </div>
      <label className="field">
        <span className="field__label">Time limit</span>
        <select value={seconds} onChange={(e) => setSeconds(Number(e.target.value))} aria-label="Time limit">
          {TIME_LIMITS.map((t) => <option key={t.seconds} value={t.seconds}>{t.label}</option>)}
        </select>
        <span className="field__hint">Ratchet also stops when {PLATEAU_EVALS} evaluations in a row bring no verified win.</span>
      </label>
      <div className="step__foot">
        <Button variant="ghost" onClick={onBack}>Back</Button>
        <Button disabled={submitting} onClick={onStart}><Icon name="play" size={12} /> {submitting ? 'Starting…' : 'Start'}</Button>
      </div>
    </Card>
  );
}

// ------------------------------------------------------------------ the page

export function NewOptimization() {
  const { open, go } = useNav();
  const [step, setStep] = useState<1 | 2 | 3>(1);
  const [reached, setReached] = useState<1 | 2 | 3>(1);
  const [source, setSource] = useState<Source | null>(null);
  const [agents, setAgents] = useState<AgentKind[]>([]);
  const [agentsTouched, setAgentsTouched] = useState(false);
  const [where, setWhere] = useState<AgentsWhere>('local');
  const [budget, setBudget] = useState(DEFAULT_BUDGET_USD);
  const [seconds, setSeconds] = useState(DEFAULT_TIME_SECONDS);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [missing, setMissing] = useState<PreflightItem[]>([]);

  const harnesses = useQuery(() => window.ratchet.detectHarnesses(), []);

  // Detected, signed-in agents are pre-selected (both when none is ready, so "what's missing" names each).
  useEffect(() => {
    if (agentsTouched || !harnesses.data) return;
    const ready = harnesses.data.filter((h) => h.installed && h.auth !== 'signed-out').map((h) => h.kind);
    setAgents(ready.length > 0 ? ready : ['claude-code', 'codex']);
  }, [harnesses.data, agentsTouched]);

  const demo = source?.kind === 'demo';
  const effectiveAgents: AgentKind[] = demo ? ['claude-code', 'codex'] : agents;
  const effectiveWhere: AgentsWhere = demo ? 'local' : where;
  const sourceKind = !source ? null : source.kind === 'demo' ? 'demo' : source.inspect.source.kind === 'local' ? 'local' : 'github';

  useEffect(() => {
    if (!sourceKind) return;
    let live = true;
    window.ratchet.preflight({ agents: effectiveAgents, agentsWhere: effectiveWhere, source: sourceKind }).then((r) => live && setMissing(r), () => undefined);
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourceKind, effectiveAgents.join(','), effectiveWhere]);

  const advance = (to: 1 | 2 | 3) => {
    setStep(to);
    setReached((r) => (to > r ? to : r));
  };

  const onSource = (s: Source | null) => {
    setSource(s);
    setReached(1);
    setStep(1);
  };

  async function start() {
    if (!source) return;
    setSubmitting(true);
    setError(null);
    try {
      const common = { agents: effectiveAgents, agentsWhere: effectiveWhere, budgetUsd: budget, timeLimitSeconds: seconds, plateauEvals: PLATEAU_EVALS };
      const id = await window.ratchet.createOptimization(
        source.kind === 'demo'
          ? { ...common, demo: source.template.id as DemoId, repoUrl: '', baseRef: 'main' }
          : { ...common, repoUrl: source.inspect.repo!, baseRef: source.inspect.baseRef, ...(source.inspect.config?.name ? { name: source.inspect.config.name } : {}) },
      );
      // The baseline run takes about a minute: show the Agents screen right away (status "Baselining") instead of
      // freezing this form, and surface a failed start there.
      window.ratchet.startOptimization(id).catch((e: unknown) => reportStartError(id, e));
      open(id, 'agents');
    } catch (e) {
      setError(errMsg(e));
      setSubmitting(false);
    }
  }

  const summaryRepo = !source ? '—' : source.kind === 'demo' ? demoTitle(source.template.title) : (source.inspect.repo ?? '—');
  const cfg = source?.kind === 'repo' ? source.inspect.config : null;
  const metricText = source?.kind === 'demo' ? source.template.objective.metric : cfg?.metric ?? null;
  const dir = source?.kind === 'demo' ? source.template.objective.direction : cfg?.direction;
  const agentNames = effectiveAgents.map((a) => AGENT_CHOICES.find((c) => c.kind === a)?.label ?? a).join(', ');
  const limitHours = seconds / 3600;
  const pageSub = useMemo(() => 'Pick a repository, check what will run, set the budget. Ratchet locks a baseline before any agent starts.', []);

  return (
    <>
      <PageHeader
        eyebrow={<Crumbs />}
        title="New optimization"
        sub={pageSub}
        actions={<Button variant="ghost" onClick={() => go('optimizations')}>Cancel</Button>}
      />
      <ErrorNote error={error} />
      <Stepper step={step} reached={reached} onGo={setStep} />

      <div className="new-layout">
        <div className="new-steps">
          {step === 1 && <RepoStep source={source} onSource={onSource} onNext={() => advance(2)} />}
          {step === 2 && source && (
            <ReviewStep source={source} agents={agents} setAgents={(a) => (setAgentsTouched(true), setAgents(a))} where={where} setWhere={setWhere}
              harnesses={harnesses.data} missing={missing} onBack={() => setStep(1)} onNext={() => advance(3)} />
          )}
          {step === 3 && source && (
            <BudgetStep budget={budget} setBudget={setBudget} seconds={seconds} setSeconds={setSeconds} submitting={submitting} onBack={() => setStep(2)} onStart={() => void start()} />
          )}
        </div>

        <aside className="new-review">
          <Card className="review">
            <h2>Summary</h2>
            <div className="kv-row"><span>Repository</span><span className="mono">{summaryRepo}</span></div>
            {source?.kind === 'repo' && <div className="kv-row"><span>Base ref</span><span className="mono">{source.inspect.baseRef}</span></div>}
            <div className="kv-row"><span>Metric</span><span className="mono">{metricText ? `${metricText} ${dir === 'minimize' ? '↓' : '↑'}` : '—'}</span></div>
            <div className="kv-row"><span>Agents</span><span>{source ? agentNames || 'none' : '—'}</span></div>
            <div className="kv-row"><span>Run on</span><span>{source ? (effectiveWhere === 'local' ? 'This Mac' : 'Cloud') : '—'}</span></div>
            <div className="kv-row"><span>Limits</span><span className="mono">${budget} · {+limitHours.toFixed(1)} h</span></div>
            <div className="callout">
              <Eyebrow>Baseline first</Eyebrow>
              <span>The baseline runs on pinned hardware and sets the noise floor before any agent starts.</span>
            </div>
          </Card>
        </aside>
      </div>
    </>
  );
}
