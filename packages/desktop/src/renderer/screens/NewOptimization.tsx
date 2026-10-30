import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import { buildConfigYaml, DEFAULT_FORM, type ConfigForm } from '../../shared/config-yaml';
import type { AgentKind, PreflightItem, ValidateConfigResult } from '../../shared/types';
import { Crumbs, ErrorNote, PageHeader } from '../components/common';
import { Icon } from '../components/icons';
import { errMsg, useNav } from '../data';

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

export interface NewOptimizationFormState {
  name: string;
  repoUrl: string;
  baseRef: string;
  configPath: string;
  form: ConfigForm;
  agents: AgentKind[];
  budgetUsd: number;
  timeLimitSeconds: number;
  plateauEvals: number;
}

export const INITIAL_STATE: NewOptimizationFormState = {
  name: '',
  repoUrl: 'https://github.com/acme/search-api',
  baseRef: 'main',
  configPath: 'ratchet.yaml',
  form: DEFAULT_FORM,
  agents: ['claude-code', 'codex'],
  budgetUsd: 100,
  timeLimitSeconds: 28_800,
  plateauEvals: 25,
};

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="field__label">{label}</span>
      {children}
      {hint && <span className="field__hint">{hint}</span>}
    </label>
  );
}

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

const lineCount = (s: string) => s.split('\n').filter((l) => l.trim() !== '').length;

export function NewOptimization() {
  const { open, go } = useNav();
  const [s, setS] = useState(INITIAL_STATE);
  const [override, setOverride] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [validation, setValidation] = useState<ValidateConfigResult | null>(null);
  const [preflight, setPreflight] = useState<PreflightItem[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const generated = useMemo(() => buildConfigYaml(s.form), [s.form]);
  const yaml = override ?? generated;

  const setForm = (patch: Partial<ConfigForm>) => setS((p) => ({ ...p, form: { ...p.form, ...patch } }));
  const set = <K extends keyof NewOptimizationFormState>(k: K, v: NewOptimizationFormState[K]) => setS((p) => ({ ...p, [k]: v }));

  // Validate through the main process (parseConfig is the single source of truth), debounced while typing.
  useEffect(() => {
    let live = true;
    const t = setTimeout(() => {
      window.ratchet.validateConfig(yaml).then((r) => live && setValidation(r), (e: unknown) => live && setError(errMsg(e)));
      window.ratchet.preflight({ agents: s.agents, configYaml: yaml }).then((r) => live && setPreflight(r), () => undefined);
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [yaml, s.agents]);

  const toggleAgent = (k: AgentKind) =>
    set('agents', s.agents.includes(k) ? s.agents.filter((a) => a !== k) : [...s.agents, k]);

  const valid = validation?.ok === true;
  const blocked = preflight.some((p) => p.state === 'fail');
  const canSubmit = valid && s.agents.length > 0 && s.repoUrl.trim() !== '' && s.baseRef.trim() !== '' && !blocked && !submitting;

  async function submit() {
    setSubmitting(true);
    setError(null);
    try {
      const id = await window.ratchet.createOptimization({
        ...(s.name.trim() && { name: s.name.trim() }),
        repoUrl: s.repoUrl.trim(),
        baseRef: s.baseRef.trim(),
        configPath: s.configPath.trim() || 'ratchet.yaml',
        configYaml: yaml,
        agents: s.agents,
        budgetUsd: s.budgetUsd,
        timeLimitSeconds: s.timeLimitSeconds,
        plateauEvals: s.plateauEvals,
      });
      await window.ratchet.startOptimization(id);
      open(id, 'agents');
    } catch (e) {
      setError(errMsg(e));
      setSubmitting(false);
    }
  }

  const f = s.form;
  const targetReady = s.repoUrl.trim() !== '' && s.baseRef.trim() !== '';
  const summary = validation?.ok ? validation.summary : null;
  const protectedList = f.protectedGlobs.split('\n').map((l) => l.trim()).filter(Boolean);
  const agentNames = s.agents.map((a) => AGENT_CHOICES.find((c) => c.kind === a)?.label ?? a).join(', ');
  const limitHours = s.timeLimitSeconds / 3600;
  return (
    <>
      <PageHeader
        eyebrow={<Crumbs />}
        title="New optimization"
        sub="Pick a benchmark, set guardrails, choose agents. Ratchet locks a baseline before any agent starts."
        actions={
          <>
            <Button variant="ghost" onClick={() => go('optimizations')}>Cancel</Button>
            <Button disabled={!canSubmit} onClick={submit}><Icon name="play" size={12} /> {submitting ? 'Starting…' : 'Run baseline & start'}</Button>
          </>
        }
      />
      <ErrorNote error={error} />

      <div className="new-layout">
        <div className="new-steps">
          <Card className="step">
            <StepHead n={1} title="Target" sub="Where agents start from" tone={targetReady ? 'ready' : 'idle'}
              aside={targetReady ? <Pill tone="mint"><Icon name="check" size={12} /> Ready</Pill> : undefined} />
            <div className="form-grid">
              <Field label="Name" hint="Optional"><input value={s.name} onChange={(e) => set('name', e.target.value)} placeholder="search-api p95 latency" /></Field>
              <Field label="Repository URL"><input value={s.repoUrl} onChange={(e) => set('repoUrl', e.target.value)} /></Field>
              <Field label="Base ref"><input className="mono" value={s.baseRef} onChange={(e) => set('baseRef', e.target.value)} /></Field>
              <Field label="Config path" hint="Path of ratchet.yaml inside the repo"><input className="mono" value={s.configPath} onChange={(e) => set('configPath', e.target.value)} /></Field>
            </div>
            <div className="file-bar">
              <span className="mono" style={{ color: 'var(--rt-color-brass-highlight)' }}>{s.configPath || 'ratchet.yaml'}</span>
              <span className="muted">
                {summary ? `${summary.metric} · ${summary.guardrails.length} guardrail${summary.guardrails.length === 1 ? '' : 's'} · ${summary.protectedGlobs.length} protected path${summary.protectedGlobs.length === 1 ? '' : 's'}` : validation ? 'needs attention — see the review panel' : 'checking…'}
              </span>
              <button type="button" className="text-link" style={{ marginLeft: 'auto' }} onClick={() => { setEditing(true); setOverride((o) => o ?? generated); }}>View file</button>
            </div>
          </Card>

          <Card className="step step--focus">
            <StepHead n={2} title="Benchmark & metric" sub="The number agents are trying to move" tone="focus" />
            <div className="form-grid">
              <Field label="Metric" hint="Key in the benchmark's JSON output"><input className="mono" value={f.metric} onChange={(e) => setForm({ metric: e.target.value })} /></Field>
              <div className="field">
                <span className="field__label">Goal</span>
                <div className="segmented" role="radiogroup" aria-label="Direction">
                  {(['minimize', 'maximize'] as const).map((d) => (
                    <button key={d} type="button" role="radio" aria-checked={f.direction === d} aria-label={d}
                      className={f.direction === d ? 'segmented__on' : ''} onClick={() => setForm({ direction: d })}>{d === 'minimize' ? 'Lower is better' : 'Higher is better'}</button>
                  ))}
                </div>
              </div>
              <Field label="Minimum gain (%)" hint="A candidate must beat the best by this much and the noise floor">
                <input className="mono" type="number" min={0} max={100} step={0.5} value={f.minGainPct} onChange={(e) => setForm({ minGainPct: Number(e.target.value) })} />
              </Field>
              <Field label="Runs per eval"><input className="mono" type="number" min={1} value={f.repeats} onChange={(e) => setForm({ repeats: Number(e.target.value) })} /></Field>
              <Field label="Bench command"><input className="mono" value={f.benchRun} onChange={(e) => setForm({ benchRun: e.target.value })} /></Field>
              <Field label="Bench ref" hint="Protected ref holding the trusted bench"><input className="mono" value={f.benchRef} onChange={(e) => setForm({ benchRef: e.target.value })} /></Field>
              <Field label="Bench hardware"><input className="mono" value={f.hardware} onChange={(e) => setForm({ hardware: e.target.value })} /></Field>
              <Field label="Confirm trace" hint="Hidden trace for the confirm run (optional)"><input className="mono" value={f.confirmTrace} onChange={(e) => setForm({ confirmTrace: e.target.value })} placeholder="s3://sealed/trace.json" /></Field>
            </div>
          </Card>

          <Card className="step">
            <StepHead n={3} title="Guardrails" sub="A candidate that fails any of these is blocked, however fast it is" tone="idle" />
            <div className="form-grid form-grid--wide">
              <Field label="Guardrails" hint="One per line: run: <command> or <metric>: <op> <value>[x]">
                <textarea className="textarea-mono" rows={5} value={f.guardrails} onChange={(e) => setForm({ guardrails: e.target.value })} />
              </Field>
              <Field label="Protected paths" hint="One glob per line; changes here block a candidate">
                <textarea className="textarea-mono" rows={5} value={f.protectedGlobs} onChange={(e) => setForm({ protectedGlobs: e.target.value })} />
              </Field>
            </div>
            {protectedList.length > 0 && (
              <div className="protected-row">Protected from agents{protectedList.map((g) => <span key={g} className="chip-tag mono" style={{ color: '#C3CBD8', padding: '4px 8px', borderRadius: 6 }}>{g}</span>)}</div>
            )}
          </Card>

          <Card className="step">
            <StepHead n={4} title="Agents & limits" sub="Each agent runs headless on its own runner and branch" tone="idle" />
            <div className="agent-choices" role="group" aria-label="Agents">
              {AGENT_CHOICES.map((a) => {
                const on = s.agents.includes(a.kind);
                return (
                  <button key={a.kind} type="button" aria-pressed={on} className={`agent-choice agent-choice--${a.kind}${on ? ' agent-choice--on' : ''}`} onClick={() => toggleAgent(a.kind)}>
                    <AgentAvatar agent={a.kind} size={38} />
                    <span className="agent-choice__text">
                      <span className="agent-choice__name">{a.label}</span>
                      <span className="agent-choice__hint mono">{a.hint}</span>
                    </span>
                    <span className={`agent-choice__check${on ? ' agent-choice__check--on' : ''}`} aria-hidden="true">{on && <Icon name="check" size={12} />}</span>
                  </button>
                );
              })}
            </div>
            <div className="budget">
              <label className="budget__top" htmlFor="budget-range">
                <span>{`Budget · $${s.budgetUsd}`}</span>
                <span className="budget__value" aria-hidden="true">${s.budgetUsd}</span>
              </label>
              <input id="budget-range" type="range" min={5} max={500} step={5} value={s.budgetUsd} aria-label="Budget" style={{ width: '100%', accentColor: '#D9B26F' }} onChange={(e) => set('budgetUsd', Number(e.target.value))} />
              <span className="budget__scale mono"><span>$5</span><span>warn at 80% · hard stop at 100%</span><span>$500</span></span>
            </div>
            <div className="form-grid">
              <Field label="Time limit">
                <select value={s.timeLimitSeconds} onChange={(e) => set('timeLimitSeconds', Number(e.target.value))} aria-label="Time limit">
                  {TIME_LIMITS.map((t) => <option key={t.seconds} value={t.seconds}>{t.label}</option>)}
                </select>
              </Field>
              <Field label="Stop after evals with no gain" hint="Plateau: evaluations without a verified win">
                <input className="mono" type="number" min={1} value={s.plateauEvals} onChange={(e) => set('plateauEvals', Number(e.target.value))} />
              </Field>
              <Field label="Allowed instance types" hint="Comma separated"><input className="mono" value={f.instanceTypes} onChange={(e) => setForm({ instanceTypes: e.target.value })} /></Field>
              <Field label="Agent brief" hint="File injected into every session"><input className="mono" value={f.instructions} onChange={(e) => setForm({ instructions: e.target.value })} /></Field>
            </div>
          </Card>
        </div>

        <aside className="new-review">
          <Card className="review">
            <h2>Review</h2>
            <div className="kv-row"><span>Repository</span><span className="mono">{s.repoUrl.replace(/^https?:\/\/github\.com\//, '') || '—'}</span></div>
            <div className="kv-row"><span>Base ref</span><span className="mono">{s.baseRef || '—'}</span></div>
            <div className="kv-row"><span>Metric</span><span className="mono">{f.metric} {f.direction === 'minimize' ? '↓' : '↑'} · min {f.minGainPct}%</span></div>
            <div className="kv-row"><span>Guardrails</span><span>{lineCount(f.guardrails)} active</span></div>
            <div className="kv-row"><span>Agents</span><span>{agentNames || 'none'}</span></div>
            <div className="kv-row"><span>Limits</span><span className="mono">${s.budgetUsd} · {+limitHours.toFixed(1)} h</span></div>
            <div className="callout">
              <Eyebrow>Baseline first</Eyebrow>
              <span>{f.repeats} runs on pinned hardware ({f.hardware || 'unspecified'}) set the noise floor before any agent starts.</span>
            </div>
          </Card>

          <Card className="yaml-card">
            <div className="yaml-card__head">
              <span className="mono yaml-card__title">ratchet.yaml</span>
              <div className="review__tabs">
                <button type="button" className={!editing ? 'on' : ''} onClick={() => setEditing(false)}>Preview</button>
                <button type="button" className={editing ? 'on' : ''} onClick={() => { setEditing(true); setOverride((o) => o ?? generated); }}>Edit</button>
                {override !== null && <button type="button" onClick={() => { setOverride(null); setEditing(false); }}>Reset</button>}
              </div>
            </div>
            {editing ? (
              <textarea className="yaml mono" aria-label="ratchet.yaml" rows={20} spellCheck={false} value={override ?? generated} onChange={(e) => setOverride(e.target.value)} />
            ) : (
              <pre className="yaml mono" aria-label="Generated ratchet.yaml">{yaml}</pre>
            )}
            <div aria-live="polite" className="validation">
              {validation === null ? (
                <Pill>Validating…</Pill>
              ) : validation.ok ? (
                <Pill tone="mint" dot>Valid config</Pill>
              ) : (
                <>
                  <Pill tone="rose" dot>{validation.errors.length} error{validation.errors.length === 1 ? '' : 's'}</Pill>
                  <ul className="errors">
                    {validation.errors.map((e, i) => (
                      <li key={i}>{e.path && <code className="mono">{e.path}</code>} {e.message}</li>
                    ))}
                  </ul>
                </>
              )}
            </div>
          </Card>

          <Card className="preflight-card">
            <Eyebrow>Preflight</Eyebrow>
            <ul className="preflight">
              {preflight.map((p) => (
                <li key={p.id} data-state={p.state}>
                  <span className={`preflight__icon preflight__icon--${p.state}`} aria-hidden="true"><Icon name={p.state === 'ok' ? 'check' : 'warn'} size={14} /></span>
                  <span>
                    <span className={`preflight__label--${p.state}`}>{p.label}</span>
                    <span className="preflight__detail">{p.detail}</span>
                  </span>
                </li>
              ))}
            </ul>
          </Card>
        </aside>
      </div>
    </>
  );
}
