import { useMemo, useState } from 'react';
import { Button, Pill } from '@ratchet/ui';
import type { RepoVM } from '../../shared/types';
import { useNav, useQuery } from '../data';
import { Icon } from './icons';

/**
 * Searchable list of the signed-in account's repositories. Without a GitHub sign-in it says so and points at Settings.
 * `onLocal` adds the "Local folder…" button (this-machine runs).
 */
export function RepoPicker({ onPick, onLocal, selected, label = 'Search repositories' }: {
  onPick(repo: RepoVM): void;
  onLocal?(): void;
  selected?: string | null;
  label?: string;
}) {
  const { go } = useNav();
  const [query, setQuery] = useState('');
  const gh = useQuery(() => window.ratchet.githubStatus(), []);
  const signedIn = gh.data?.signedIn === true;
  const repos = useQuery(() => window.ratchet.listRepos(), [signedIn], { skip: !signedIn });
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = repos.data ?? [];
    return (q ? all.filter((r) => r.fullName.toLowerCase().includes(q) || (r.description ?? '').toLowerCase().includes(q)) : all).slice(0, 60);
  }, [repos.data, query]);

  return (
    <div className="repo-picker">
      <div className="repo-picker__bar">
        <input type="search" aria-label={label} placeholder={signedIn ? 'Search your repositories' : 'Sign in with GitHub to search your repositories'} value={query} disabled={!signedIn}
          onChange={(e) => setQuery(e.target.value)} />
        {onLocal && <Button variant="ghost" onClick={onLocal}><Icon name="plus" size={11} /> Local folder…</Button>}
      </div>
      {gh.data && !signedIn && (
        <p className="muted" role="status">
          You are not signed in to GitHub. <button type="button" className="text-link" onClick={() => go('settings')}>Open Settings</button> to sign in{onLocal ? ', or choose a local folder' : ''}.
        </p>
      )}
      {signedIn && repos.error && <p role="alert" className="error-note">{repos.error}</p>}
      {signedIn && repos.loading && !repos.data && <p className="muted">Loading your repositories…</p>}
      {signedIn && repos.data && (
        <ul className="repo-list" aria-label="Repositories">
          {shown.map((r) => (
            <li key={r.fullName}>
              <button type="button" className={`repo-row${selected === r.fullName ? ' repo-row--on' : ''}`} aria-pressed={selected === r.fullName} onClick={() => onPick(r)}>
                <span className="repo-row__main">
                  <span className="repo-row__name mono">{r.fullName}</span>
                  {r.description && <span className="repo-row__desc">{r.description}</span>}
                </span>
                {r.private && <Pill>Private</Pill>}
                <span className="repo-row__branch mono faint">{r.defaultBranch}</span>
              </button>
            </li>
          ))}
          {shown.length === 0 && <li className="muted repo-list__empty">{repos.data.length === 0 ? 'No repositories on this account.' : 'No repository matches.'}</li>}
        </ul>
      )}
    </div>
  );
}
