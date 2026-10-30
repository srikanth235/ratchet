import { renderTemplate, type RenderTemplateOptions, type TemplateFile } from './render.js';
import { templateDefs, type TemplateInfo } from './registry.js';

/** Everything needed to commit a template to a repository and open the PR. Pure data: no git, no network. */
export interface TemplateProposal {
  templateId: string;
  info: TemplateInfo;
  /** Suggested branch name (the caller may add a suffix when it already exists). */
  branch: string;
  commitMessage: string;
  title: string;
  /** Markdown: what was added, what to fill in, how to run a baseline. */
  body: string;
  files: TemplateFile[];
}

function purpose(path: string, info: TemplateInfo): string {
  if (path === 'ratchet.yaml') return 'the optimization config: objective, evaluator template, guardrails';
  if (path === 'RATCHET.md') return 'the brief every agent session starts from';
  if (path.startsWith('bench/') || path.startsWith('eval/') || path.startsWith('data/')) {
    return path.endsWith('.json') || path.endsWith('.jsonl') ? 'evaluator input you tune (protected)' : 'the trusted evaluator (protected)';
  }
  if (path.startsWith('src/')) return `the placeholder code the agents will improve (${info.category === 'forecasting' ? 'your forecaster' : 'replace with yours'})`;
  return 'template file';
}

/**
 * Describe the PR that adds `templateId` to a repository: branch, commit message, title and a body that says what was
 * added, what still needs filling in, and how to run a baseline. Throws for an unknown template or a demo.
 */
export function buildTemplateProposal(templateId: string, options: RenderTemplateOptions = {}): TemplateProposal {
  const { info, files, options: o } = renderTemplate(templateId, options);
  const def = templateDefs().find((d) => d.id === info.id)!;
  const rows = files.map((f) => `| \`${f.path}\` | ${purpose(f.path, info)} |`).join('\n');
  const evaluatorCmd = def.run ?? '';
  const body = `## Add the Ratchet "${info.title}" template

This adds what Ratchet needs to ${info.description}: a trusted ${info.evaluatorKind} that agents cannot edit, the config and a brief. It only adds files; it does not change your code.

**Objective:** ${o.direction} \`${o.metric}\` (${info.objective.unit}).
**Measures:** ${info.measures}.${info.holdout ? `\n**Holdout:** every win is re-checked on a ${info.holdout} that the agents never see.` : ''}
**Bench machine:** \`${info.machine}\`.

### What was added

| File | Purpose |
|---|---|
${rows}

\`ratchet.yaml\`, the evaluator files above, lockfiles and install config are protected automatically: a candidate that edits them is blocked.

### Before you start an optimization

- Fill in the \`TODO\`s: the evaluator's config (${files
    .filter((f) => /\.(config|json)$|\.jsonl$/.test(f.path) && !f.path.endsWith('package.json'))
    .map((f) => `\`${f.path}\``)
    .join(', ') || 'see the evaluator file header'}), the guardrail test command in \`ratchet.yaml\`, and the brief in \`RATCHET.md\`.
- Replace the placeholder metric value (\`objective.value\`) with your own numbers if you want a dollar figure on the result.

### Run a baseline

\`\`\`sh
${evaluatorCmd}          # the evaluator on its own: prints the JSON that contains \`${o.metric}\`
ratchet validate         # checks ratchet.yaml
ratchet baseline         # runs the evaluator several times: prints the noise and the gain a win must beat
\`\`\`

In the Ratchet app: New optimization, pick this repository, and the Review step runs the baseline and shows the
noise and the gain a win has to beat.
`;
  return {
    templateId: info.id,
    info,
    branch: `ratchet/template-${info.id}`,
    commitMessage: `Add Ratchet ${info.title} template\n\nAdds ratchet.yaml, RATCHET.md and the ${info.id} evaluator files.`,
    title: `Add Ratchet "${info.title}" template`,
    body,
    files,
  };
}
