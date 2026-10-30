import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Button, Card, Eyebrow, Pill } from '@ratchet/ui';
import { buildConfigYaml, DEFAULT_FORM, type ConfigForm } from '../../shared/config-yaml';
import type { AgentKind, PreflightItem, ValidateConfigResult } from '../../shared/types';
import { ErrorNote, PageHeader } from '../components/common';
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
  return (
    <>
      <PageHeader
        title="New optimization"
        sub="Point agents at a benchmark. Ratchet runs a baseline, then only keeps verified wins."
        actions={<Button variant="ghost" onClick={() => go('optimizations')}>Cancel</Button>}
      />

      <div className="new-layout">
        <div className="new-steps">
          <Card className="step">
            <Eyebrow>1 · Target</Eyebrow>
            <div className="form-grid">
              <Field label="Name" hint="Optional"><input value={s.name} onChange={(e) => set('name', e.target.value)} placeholder="search-api p95 latency" /></Field>
              <Field label="Repository URL"><input value={s.repoUrl} onChange={(e) => set('repoUrl', e.target.value)} /></Field>
              <Field label="Base ref"><input className="mono" value={s.baseRef} onChange={(e) => set('baseRef', e.target.value)} /></Field>
              <Field label="Config path" hint="Path of ratchet.yaml inside the repo"><input className="mono" value={s.configPath} onChange={(e) => set('configPath', e.target.value)} /></Field>
            </div>
          </Card>

          <Card className="step">
            <Eyebrow>2 · Benchmark &amp; metric</Eyebrow>
            <div className="form-grid">
              <Field label="Metric" hint="Key in the benchmark's JSON output"><input className="mono" value={f.metric} onChange={(e) => setForm({ metric: e.target.value })} /></Field>
              <Field label="Direction">
                <div className="segmented" role="radiogroup" aria-label="Direction">
                  {(['minimize', 'maximize'] as const).map((d) => (
                    <button key={d} type="button" role="radio" aria-checked={f.direction === d}
                      className={f.direction === d ? 'segmented__on' : ''} onClick={() => setForm({ direction: d })}>{d}</button>
                  ))}
                </div>
              </Field>
              <Field label="Minimum gain (%)" hint="A candidate must beat the best by this much and the noise floor">
                <input type="number" min={0} max={100} step={0.5} value={f.minGainPct} onChange={(e) => setForm({ minGainPct: Number(e.target.value) })} />
              </Field>
              <Field label="Repeats"><input type="number" min={1} value={f.repeats} onChange={(e) => setForm({ repeats: Number(e.target.value) })} /></Field>
              <Field label="Bench command"><input className="mono" value={f.benchRun} onChange={(e) => setForm({ benchRun: e.target.value })} /></Field>
              <Field label="Bench ref" hint="Protected ref holding the trusted bench"><input className="mono" value={f.benchRef} onChange={(e) => setForm({ benchRef: e.target.value })} /></Field>
              <Field label="Hardware"><input className="mono" value={f.hardware} onChange={(e) => setForm({ hardware: e.target.value })} /></Field>
              <Field label="Confirm trace" hint="Hidden trace for the confirm run (optional)"><input className="mono" value={f.confirmTrace} onChange={(e) => setForm({ confirmTrace: e.target.value })} placeholder="s3://sealed/trace.json" /></Field>
            </div>
          </Card>

          <Card className="step">
            <Eyebrow>3 · Guardrails</Eyebrow>
            <div className="form-grid">
              <Field label="Guardrails" hint="One per line: run: <command> or <metric>: <op> <value>[x]">
                <textarea className="mono" rows={5} value={f.guardrails} onChange={(e) => setForm({ guardrails: e.target.value })} />
              </Field>
              <Field label="Protected paths" hint="One glob per line; changes here block a candidate">
                <textarea className="mono" rows={5} value={f.protectedGlobs} onChange={(e) => setForm({ protectedGlobs: e.target.value })} />
              </Field>
            </div>
          </Card>

          <Card className="step">
            <Eyebrow>4 · Agents &amp; limits</Eyebrow>
            <div className="agent-choices" role="group" aria-label="Agents">
              {AGENT_CHOICES.map((a) => (
                <button key={a.kind} type="button" aria-pressed={s.agents.includes(a.kind)}
                  className={`agent-choice${s.agents.includes(a.kind) ? ' agent-choice--on' : ''}`} onClick={() => toggleAgent(a.kind)}>
                  <span className="agent-choice__name">{a.label}</span>
                  <span className="agent-choice__hint">{a.hint}</span>
                </button>
              ))}
            </div>
            <div className="form-grid">
              <Field label={`Budget · $${s.budgetUsd}`} hint="Agents stop when spend reaches the budget">
                <input type="range" min={5} max={500} step={5} value={s.budgetUsd} aria-label="Budget" onChange={(e) => set('budgetUsd', Number(e.target.value))} />
              </Field>
              <Field label="Time limit">
                <select value={s.timeLimitSeconds} onChange={(e) => set('timeLimitSeconds', Number(e.target.value))} aria-label="Time limit">
                  {TIME_LIMITS.map((t) => <option key={t.seconds} value={t.seconds}>{t.label}</option>)}
                </select>
              </Field>
              <Field label="Plateau (evaluations)" hint="Stop after this many evaluations without a verified win">
                <input type="number" min={1} value={s.plateauEvals} onChange={(e) => set('plateauEvals', Number(e.target.value))} />
              </Field>
              <Field label="Allowed instance types" hint="Comma separated"><input className="mono" value={f.instanceTypes} onChange={(e) => setForm({ instanceTypes: e.target.value })} /></Field>
              <Field label="Agent brief" hint="File injected into every session"><input className="mono" value={f.instructions} onChange={(e) => setForm({ instructions: e.target.value })} /></Field>
            </div>
          </Card>
        </div>

        <aside className="new-review">
          <Card className="review">
            <div className="review__head">
              <Eyebrow>ratchet.yaml</Eyebrow>
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

          <Card className="review">
            <Eyebrow>Preflight</Eyebrow>
            <ul className="preflight">
              {preflight.map((p) => (
                <li key={p.id} data-state={p.state}>
                  <span className={`dot dot--${p.state === 'ok' ? 'mint' : p.state === 'warn' ? 'brass' : 'rose'}`} aria-hidden="true" />
                  <span className="preflight__label">{p.label}</span>
                  <span className="preflight__detail muted">{p.detail}</span>
                </li>
              ))}
            </ul>
            <ErrorNote error={error} />
            <Button disabled={!canSubmit} onClick={submit}>{submitting ? 'Starting…' : 'Run baseline & start'}</Button>
          </Card>
        </aside>
      </div>
    </>
  );
}
