import { useState } from 'react';
import { Button, Card, Eyebrow } from '@ratchet/ui';
import type { ProposeTemplateVM, RepoVM, TemplateVM } from '../../shared/types';
import { errMsg } from '../data';
import { ErrorNote } from './common';
import { RepoPicker } from './RepoPicker';

/** A link that opens in the system browser (the main process denies in-app windows and opens https URLs externally). */
export function ExternalLink({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <a href={href} className="text-link" target="_blank" rel="noreferrer noopener">{children}</a>
  );
}

/** The result of a template pull request: where it is and what it added. */
export function PrResult({ r }: { r: ProposeTemplateVM }) {
  return (
    <div className="pr-result" role="status" data-testid="pr-result">
      <span className="pr-result__title">Pull request opened</span>
      <ExternalLink href={r.prUrl}>{r.prUrl}</ExternalLink>
      <span className="muted">Adds {r.files.join(', ')} on <span className="mono">{r.branch}</span>. Merge it, then pick the repository again.</span>
    </div>
  );
}

/** Propose `template` on `repo` (`owner/name`): commits its files on a branch and opens the pull request. */
export function useProposeTemplate() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ProposeTemplateVM | null>(null);
  const run = async (repo: string, baseRef: string, template: TemplateVM): Promise<void> => {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      setResult(await window.ratchet.proposeTemplate({ repoUrl: repo, baseRef, templateId: template.id }));
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, result, run, reset: () => (setResult(null), setError(null)) };
}

/** "Use template": pick a repository, then Ratchet opens a pull request adding the template's files. */
export function UseTemplatePanel({ template, onClose }: { template: TemplateVM; onClose(): void }) {
  const [repo, setRepo] = useState<RepoVM | null>(null);
  const p = useProposeTemplate();
  return (
    <Card className="use-template" data-testid="use-template">
      <div className="row-between">
        <Eyebrow>Use “{template.title}” on a repository</Eyebrow>
        <button type="button" className="text-link" onClick={onClose}>Close</button>
      </div>
      <p className="muted">Ratchet opens a pull request on the repository that adds {template.files.length > 0 ? <span className="mono">{template.files.join(', ')}</span> : 'the template'}. Nothing changes until you merge it.</p>
      <RepoPicker onPick={(r) => { setRepo(r); p.reset(); }} selected={repo?.fullName ?? null} label="Search repositories for the template" />
      <ErrorNote error={p.error} />
      {p.result ? (
        <PrResult r={p.result} />
      ) : (
        <div className="row-between">
          <span className="muted">{repo ? <>Target: <span className="mono">{repo.fullName}</span> ({repo.defaultBranch})</> : 'Choose a repository.'}</span>
          <Button disabled={!repo || p.busy} onClick={() => repo && void p.run(repo.fullName, repo.defaultBranch, template)}>{p.busy ? 'Opening the pull request…' : 'Open pull request'}</Button>
        </div>
      )}
    </Card>
  );
}
