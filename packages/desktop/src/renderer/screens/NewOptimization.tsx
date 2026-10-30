import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AgentAvatar, Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import { buildConfigYaml, DEFAULT_FORM, EVALUATOR_KINDS, formFromYaml, patchConfigYaml, type ConfigForm, type EvaluatorKind } from '../../shared/config-yaml';
import type { AgentKind, PreflightItem, ValidateConfigResult } from '../../shared/types';
import { Crumbs, ErrorNote, PageHeader } from '../components/common';
import { Icon } from '../components/icons';
import { errMsg, reportStartError, useNav } from '../data';
import { fmtUsdCompact } from '../format';

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

/** What each evaluator kind is for. `evaluator.kind` is informational for the engine; packs set it. */
const EVALUATOR_CHOICES: Record<EvaluatorKind, { tag: string; label: string; measures: string; hint: string }> = {
  benchmark: { tag: 'bm', label: 'Benchmark', measures: 'latency · throughput · memory', hint: 'Load test or micro-benchmark on pinned hardware' },
  backtest: { tag: 'bt', label: 'Backtest', measures: 'WAPE · MAPE · bias', hint: 'Rolling-origin backtest, sealed recent period as holdout' },
  eval: { tag: 'ev', label: 'Eval set', measures: 'accuracy · pass rate · score', hint: 'Scored evaluation set, held-out partition as holdout' },
  simulation: { tag: 'sim', label: 'Simulation', measures: 'cost · reward · service level', hint: 'Simulated environment with a sealed scenario seed' },
};

export interface NewOptimizationFormState {
  name: string;
  repoUrl: string;
  baseRef: string;
  configPath: string;
  /** The ratchet.yaml text: the single source of truth the form fields and the editor both edit. */
  yaml: string;
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
  yaml: buildConfigYaml(DEFAULT_FORM),
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

/** `run: pnpm test` / `error_rate: < 0.001` / `peak_rss: < 1.2x` → what the tick list shows. */
function describeGuardrail(line: string): { title: string; sub: string; tag: string } {
  const i = line.indexOf(':');
  const key = i > 0 ? line.slice(0, i).trim() : line.trim();
  const val = i > 0 ? line.slice(i + 1).trim() : '';
  if (key === 'run') return { title: val || '(empty command)', sub: 'command must exit 0 on every candidate', tag: 'run' };
  const rel = /x\s*$/.test(val);
  return {
    title: `${key} ${rel ? val.replace(/\s*x\s*$/, '× baseline') : val}`.trim(),
    sub: rel ? "metric from the evaluator's output, relative to the baseline" : "metric from the evaluator's output, absolute bound",
    tag: 'threshold',
  };
}

export function NewOptimization() {
  const { open, go } = useNav();
  const [s, setS] = useState(INITIAL_STATE);
  const [editing, setEditing] = useState(false);
  const [validation, setValidation] = useState<ValidateConfigResult | null>(null);
  const [preflight, setPreflight] = useState<PreflightItem[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newGuardrail, setNewGuardrail] = useState('');

  const yaml = s.yaml;
  // The form is read from the text; while the text does not parse, the last readable form stays on screen (disabled).
  const parsed = useMemo(() => formFromYaml(yaml), [yaml]);
  const lastForm = useRef<ConfigForm>(parsed ?? DEFAULT_FORM);
  if (parsed) lastForm.current = parsed;
  const f = parsed ?? lastForm.current;
  const formLocked = parsed === null;

  // Every guardrail seen in this session stays in the tick list, so unticking one (which removes it from the yaml) can be undone.
  const [knownGuardrails, setKnownGuardrails] = useState<string[]>(DEFAULT_FORM.guardrails);
  useEffect(() => {
    if (parsed) setKnownGuardrails((k) => [...k, ...parsed.guardrails.filter((g) => !k.includes(g))]);
  }, [parsed]);

  const setYaml = (text: string) => setS((p) => ({ ...p, yaml: text }));
  const setForm = (patch: Partial<ConfigForm>) => setS((p) => ({ ...p, yaml: patchConfigYaml(p.yaml, patch) }));
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

  const toggleGuardrail = (g: string) => {
    if (f.guardrails.includes(g)) setForm({ guardrails: f.guardrails.filter((x) => x !== g) });
    else setForm({ guardrails: knownGuardrails.filter((x) => x === g || f.guardrails.includes(x)) });
  };
  const addGuardrail = () => {
    const g = newGuardrail.trim();
    if (!g) return;
    if (!f.guardrails.includes(g)) setForm({ guardrails: [...f.guardrails, g] });
    setNewGuardrail('');
  };

  const valid = validation?.ok === true;
  const summary = validation?.ok ? validation.summary : null;
  const blocked = preflight.some((p) => p.state === 'fail');
  const canSubmit = valid && s.agents.length > 0 && s.repoUrl.trim() !== '' && s.baseRef.trim() !== '' && !blocked && !submitting;

  async function submit() {
    setSubmitting(true);
    setError(null);
    try {
      const name = s.name.trim() || summary?.name;
      const id = await window.ratchet.createOptimization({
        ...(name && { name }),
        repoUrl: s.repoUrl.trim(),
        baseRef: s.baseRef.trim(),
        configPath: s.configPath.trim() || 'ratchet.yaml',
        configYaml: yaml,
        agents: s.agents,
        budgetUsd: s.budgetUsd,
        timeLimitSeconds: s.timeLimitSeconds,
        plateauEvals: s.plateauEvals,
      });
      // The baseline run takes about a minute: show the Agents screen right away (status "Baselining") instead of
      // freezing this form, and surface a failed start there.
      window.ratchet.startOptimization(id).catch((e: unknown) => reportStartError(id, e));
      open(id, 'agents');
    } catch (e) {
      setError(errMsg(e));
      setSubmitting(false);
    }
  }

  const targetReady = s.repoUrl.trim() !== '' && s.baseRef.trim() !== '';
  const valueText = summary?.value
    ? `${fmtUsdCompact(summary.value.usdPerUnit)}${summary.value.unit ? ` per ${summary.value.unit}` : ''} / ${summary.value.period} · ${summary.value.basis}`
    : null;
  const protectedList = f.protectedGlobs.split('\n').map((l) => l.trim()).filter(Boolean);
  const agentNames = s.agents.map((a) => AGENT_CHOICES.find((c) => c.kind === a)?.label ?? a).join(', ');
  const limitHours = s.timeLimitSeconds / 3600;
  const guardRows = [...knownGuardrails, ...f.guardrails.filter((g) => !knownGuardrails.includes(g))];
  return (
    <>
      <PageHeader
        eyebrow={<Crumbs />}
        title="New optimization"
        sub="Pick an evaluator, set guardrails, choose agents. Ratchet locks a baseline before any agent starts."
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
              <Field label="Name" hint="Optional"><input value={s.name} onChange={(e) => set('name', e.target.value)} placeholder={summary?.name ?? 'search-api p95 latency'} /></Field>
              <Field label="Repository URL"><input value={s.repoUrl} onChange={(e) => set('repoUrl', e.target.value)} /></Field>
              <Field label="Base ref"><input className="mono" value={s.baseRef} onChange={(e) => set('baseRef', e.target.value)} /></Field>
              <Field label="Config path" hint="Path of ratchet.yaml inside the repo"><input className="mono" value={s.configPath} onChange={(e) => set('configPath', e.target.value)} /></Field>
            </div>
            <div className="file-bar">
              <span className="mono" style={{ color: 'var(--rt-color-brass-highlight)' }}>{s.configPath || 'ratchet.yaml'}</span>
              <span className="muted">
                {summary
                  ? [
                      summary.metric,
                      `${summary.evaluator.kind} evaluator`,
                      valueText,
                      `${summary.guardrails.length} guardrail${summary.guardrails.length === 1 ? '' : 's'}`,
                      `${summary.protectedGlobs.length} protected path${summary.protectedGlobs.length === 1 ? '' : 's'}`,
                    ].filter(Boolean).join(' · ')
                  : validation ? 'needs attention — see the review panel' : 'checking…'}
              </span>
              <button type="button" className="text-link" style={{ marginLeft: 'auto' }} onClick={() => setEditing(true)}>View file</button>
            </div>
          </Card>

          <fieldset className="form-lock" disabled={formLocked}>
          {formLocked && <p className="error-note" role="status">ratchet.yaml does not parse as YAML: fix it in the editor and the form follows.</p>}
          <Card className="step step--focus">
            <StepHead n={2} title="Evaluator & metric" sub="The trusted measurement, and the number agents are trying to move" tone="focus" />
            <div className="eval-choices" role="radiogroup" aria-label="Evaluator kind">
              {EVALUATOR_KINDS.map((k) => {
                const c = EVALUATOR_CHOICES[k];
                const on = f.evaluatorKind === k;
                return (
                  <label key={k} className={`eval-choice${on ? ' eval-choice--on' : ''}`} data-kind={k}>
                    <span className="eval-choice__top">
                      <span className="mono eval-choice__tag" aria-hidden="true">{c.tag}</span>
                      <span className="eval-choice__name">{c.label}</span>
                      <input type="radio" name="evaluator" value={k} checked={on} onChange={() => setForm({ evaluatorKind: k })} aria-label={c.label} />
                    </span>
                    <span className="mono eval-choice__run" title={on ? f.evaluatorRun : undefined}>{on ? f.evaluatorRun || 'no command yet' : c.measures}</span>
                    <span className="eval-choice__hint">{c.hint}</span>
                  </label>
                );
              })}
            </div>
            <div className="form-grid">
              <Field label="Metric" hint="Key in the evaluator's JSON output"><input className="mono" value={f.metric} onChange={(e) => setForm({ metric: e.target.value })} /></Field>
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
              <Field label="Evaluator command"><input className="mono" value={f.evaluatorRun} onChange={(e) => setForm({ evaluatorRun: e.target.value })} /></Field>
              <Field label="Evaluator ref" hint="Protected ref holding the trusted evaluator"><input className="mono" value={f.evaluatorRef} onChange={(e) => setForm({ evaluatorRef: e.target.value })} /></Field>
              <Field label="Evaluator hardware"><input className="mono" value={f.hardware} onChange={(e) => setForm({ hardware: e.target.value })} /></Field>
              <Field label="Sealed holdout" hint="Hidden trace / unseen period for the confirm run (optional)"><input className="mono" value={f.holdout} onChange={(e) => setForm({ holdout: e.target.value })} placeholder="s3://sealed/trace.json" /></Field>
            </div>
            <div className="value-row">
              <span className="field__label">Business value <span className="faint">· optional, turns the gain into $ / year</span></span>
              <div className="form-grid">
                <Field label="USD per unit of improvement">
                  <input className="mono" type="number" min={0} step="any" value={f.valueUsdPerUnit ?? ''} placeholder="none"
                    onChange={(e) => setForm({ valueUsdPerUnit: e.target.value === '' || !(Number(e.target.value) > 0) ? null : Number(e.target.value) })} />
                </Field>
                <Field label="Unit" hint="e.g. ms, WAPE point"><input className="mono" value={f.valueUnit} disabled={f.valueUsdPerUnit === null} onChange={(e) => setForm({ valueUnit: e.target.value })} /></Field>
                <Field label="Per">
                  <select value={f.valuePeriod} disabled={f.valueUsdPerUnit === null} onChange={(e) => setForm({ valuePeriod: e.target.value as 'month' | 'year' })} aria-label="Value period">
                    <option value="year">year</option>
                    <option value="month">month</option>
                  </select>
                </Field>
                <Field label="Basis">
                  <select value={f.valueBasis} disabled={f.valueUsdPerUnit === null} onChange={(e) => setForm({ valueBasis: e.target.value as 'estimated' | 'measured' })} aria-label="Value basis">
                    <option value="estimated">estimated</option>
                    <option value="measured">measured</option>
                  </select>
                </Field>
              </div>
            </div>
          </Card>

          <Card className="step">
            <StepHead n={3} title="Guardrails" sub="A candidate that fails any of these is blocked, however fast it is" tone="idle" />
            <ul className="guardrail-list" aria-label="Guardrails">
              {guardRows.map((g) => {
                const d = describeGuardrail(g);
                const on = f.guardrails.includes(g);
                return (
                  <li key={g}>
                    <label className={`guardrail${on ? '' : ' guardrail--off'}`}>
                      <input type="checkbox" checked={on} onChange={() => toggleGuardrail(g)} aria-label={g} />
                      <span className="guardrail__text">
                        <span className={`guardrail__title${d.tag === 'run' ? ' mono' : ''}`}>{d.title}</span>
                        <span className="guardrail__sub">{d.sub}</span>
                      </span>
                      <span className="mono guardrail__tag">{d.tag}</span>
                    </label>
                  </li>
                );
              })}
              <li className="guardrail guardrail--add">
                <input className="mono" value={newGuardrail} onChange={(e) => setNewGuardrail(e.target.value)} placeholder="run: <command>  or  <metric>: < value[x]" aria-label="New guardrail"
                  onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addGuardrail(); } }} />
                <Button variant="ghost" disabled={!newGuardrail.trim()} onClick={addGuardrail}><Icon name="plus" size={11} /> Add</Button>
              </li>
            </ul>
            <div className="form-grid form-grid--wide">
              <Field label="Protected paths" hint="One glob per line; changes here block a candidate">
                <textarea className="textarea-mono" rows={4} value={f.protectedGlobs} onChange={(e) => setForm({ protectedGlobs: e.target.value })} />
              </Field>
            </div>
            {protectedList.length > 0 && (
              <div className="protected-row">Protected from agents{protectedList.map((g) => <span key={g} className="chip-tag mono" style={{ color: '#C3CBD8', padding: '4px 8px', borderRadius: 6 }}>{g}</span>)}</div>
            )}
          </Card>
          </fieldset>

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
            <fieldset className="form-lock" disabled={formLocked}>
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
            </fieldset>
          </Card>
        </div>

        <aside className="new-review">
          <Card className="review">
            <h2>Review</h2>
            <div className="kv-row"><span>Repository</span><span className="mono">{s.repoUrl.replace(/^https?:\/\/github\.com\//, '') || '—'}</span></div>
            <div className="kv-row"><span>Base ref</span><span className="mono">{s.baseRef || '—'}</span></div>
            <div className="kv-row"><span>Metric</span><span className="mono">{f.metric} {f.direction === 'minimize' ? '↓' : '↑'} · min {f.minGainPct}%</span></div>
            {summary && <div className="kv-row"><span>Evaluator</span><span className="mono">{summary.evaluator.kind} · {summary.evaluator.ref}</span></div>}
            {valueText && <div className="kv-row"><span>Value</span><span className="mono">{valueText}</span></div>}
            <div className="kv-row"><span>Guardrails</span><span>{f.guardrails.length} active</span></div>
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
                <button type="button" className={editing ? 'on' : ''} onClick={() => setEditing(true)}>Edit</button>
                {yaml !== INITIAL_STATE.yaml && <button type="button" onClick={() => setYaml(INITIAL_STATE.yaml)}>Reset</button>}
              </div>
            </div>
            {editing ? (
              <textarea className="yaml mono" aria-label="ratchet.yaml" rows={20} spellCheck={false} value={yaml} onChange={(e) => setYaml(e.target.value)} />
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
