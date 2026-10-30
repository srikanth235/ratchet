import { TEMPLATE_FILES } from './files.generated.js';
import { getTemplate, resolveTemplateId, templateDefs, type TemplateDirection, type TemplateInfo } from './registry.js';

/** A file to be written into a repository. `path` is repo-relative POSIX. */
export interface TemplateFile {
  path: string;
  content: string;
}

/** What the user can tune when a template is turned into files. All optional: the template's defaults apply. */
export interface RenderTemplateOptions {
  /** Objective metric, a dotted path into the evaluator's JSON output. */
  metric?: string;
  direction?: TemplateDirection;
  /** Placeholder USD per unit of improvement per year (`objective.value`). */
  valueUsd?: number;
}

export interface RenderedTemplate {
  info: TemplateInfo;
  /** The options after defaults, as written to ratchet.yaml. */
  options: Required<RenderTemplateOptions>;
  /** ratchet.yaml first, then RATCHET.md and the evaluator files. */
  files: TemplateFile[];
}

/** A YAML scalar: plain when it is safe, JSON-quoted (valid YAML) otherwise. */
function scalar(s: string): string {
  return /^[A-Za-z_][\w.-]*$/.test(s) ? s : JSON.stringify(s);
}

const FALLBACK_TYPES = ['c7i.large', 'c7i.xlarge', 'm7i.large'];

function yamlFor(info: TemplateInfo, o: Required<RenderTemplateOptions>): string {
  const def = templateDefs().find((d) => d.id === info.id)!;
  const guardrails: string[] = [];
  if (def.testCommand) guardrails.push('  - run: npm test                  # TODO: your test command; must exit 0');
  for (const g of def.guardrails ?? []) {
    for (const [k, v] of Object.entries(g)) guardrails.push(`  - ${k}: ${JSON.stringify(v)}`);
  }
  const types = [...new Set([info.machine, ...FALLBACK_TYPES])];
  return `version: 0
pack: ${info.category}${' '.repeat(Math.max(1, 27 - info.category.length))}# template category (docs/packs/${info.category}.md)
objective:
  name: ${scalar(o.metric)}
  metric: ${scalar(o.metric)}      # key in the evaluator's JSON output (dotted path)
  direction: ${o.direction}
  # min_gain, the number of repeats and the noise floor are calibrated from the baseline's noise. To pin the win
  # threshold yourself, add \`min_gain: 0.03\` (a fraction) here and \`repeats: 5\` under evaluator.
  value:                           # business case: what one unit of the metric is worth
    usd_per_unit: ${o.valueUsd}    # USD per ${info.objective.unit} per year. A placeholder: replace it with your own numbers
    period: year
    basis: estimated               # becomes \`measured\` once confirmed in production
    unit: ${scalar(info.objective.unit)}
evaluator:
  template: ${info.id}${' '.repeat(Math.max(1, 29 - info.id.length))}# fills in the command, kind and default guardrails; its files are protected automatically
${info.holdout ? `  # holdout: s3://sealed/${info.id}-holdout.json  # TODO: the ${info.holdout}, never shown to the agents (the winner is re-checked on it)\n` : ''}environment:
  machine: ${info.machine}${' '.repeat(Math.max(1, 21 - info.machine.length))}# bench hardware (pinned, dedicated host)
guardrails:
${guardrails.join('\n')}
protected:                         # added to what is protected automatically (ratchet.yaml, the evaluator, lockfiles)
  - package.json                   # defines the guardrail commands
agent:
  instructions: RATCHET.md         # brief injected into every session
  allowed_instance_types: [${types.join(', ')}]
`;
}

/**
 * Turn a template into the files to commit: `ratchet.yaml` (the fragment, with the template's objective), the
 * `RATCHET.md` brief and the evaluator's real files. A legacy `--pack` name is accepted. Throws for an unknown id
 * and for a demo (demos have no files: they point at a project under `examples/`).
 */
export function renderTemplate(idOrAlias: string, options: RenderTemplateOptions = {}): RenderedTemplate {
  const info = getTemplate(idOrAlias);
  if (!info) throw new Error(`unknown template ${JSON.stringify(idOrAlias)}`);
  if (info.kind === 'demo') throw new Error(`${info.id} is a demo (see ${info.example}); it has no files to add`);
  const o: Required<RenderTemplateOptions> = {
    metric: options.metric?.trim() || info.objective.metric,
    direction: options.direction ?? info.objective.direction,
    valueUsd: options.valueUsd ?? info.defaultValueUsd,
  };
  const table = TEMPLATE_FILES[resolveTemplateId(idOrAlias)] ?? {};
  const fill = (text: string) =>
    text
      .replaceAll('{{metric}}', o.metric)
      .replaceAll('{{direction}}', o.direction)
      .replaceAll('{{verb}}', o.direction === 'minimize' ? 'lower' : 'raise');
  const files: TemplateFile[] = [{ path: 'ratchet.yaml', content: yamlFor(info, o) }];
  for (const [path, content] of Object.entries(table)) {
    files.push({ path, content: path === 'RATCHET.md' ? fill(content) : content });
  }
  return { info, options: o, files };
}

/** The repo paths a template adds, `ratchet.yaml` included (demos: none). */
export function templateFilePaths(idOrAlias: string): string[] {
  const id = resolveTemplateId(idOrAlias);
  const def = templateDefs().find((d) => d.id === id);
  if (!def || def.kind !== 'template') return [];
  return ['ratchet.yaml', ...Object.keys(TEMPLATE_FILES[id] ?? {})];
}
