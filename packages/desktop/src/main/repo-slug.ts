// Pure helpers shared by the service (jsdom tests import it) and the engine composition.

/** `https://github.com/acme/api.git`, `acme/api`, `git@github.com:acme/api` -> `acme/api`. A filesystem path is not a repo slug. */
export function slugOf(repo: string): string | null {
  const m = /^(?:(?:https?:\/\/|ssh:\/\/git@|git@)(?:[^/:]+)[/:])?([A-Za-z0-9][A-Za-z0-9-]*\/[A-Za-z0-9._-]+?)(?:\.git)?\/?$/.exec(repo.trim());
  return m && !repo.trim().startsWith('/') ? m[1]! : null;
}

/** The runtime's own wording mentions screens that no longer exist; say what to do in the new model. */
export function friendlyMissing(missing: string[]): string[] {
  return missing.map((m) => {
    if (/GitHub repository and token/.test(m)) return 'Sign in with GitHub (Settings)';
    if (/bench AMI/.test(m)) return 'The evaluator image for this app version was not found in your AWS account (connect AWS in Settings, or build the image)';
    if (/runner AMI/.test(m)) return 'The cloud runner image was not found in your AWS account (use "This Mac", or build the image)';
    if (/install and sign in to Claude Code or Codex/.test(m)) return 'Install and sign in to Claude Code or Codex on this machine (Settings > Agents)';
    return m;
  });
}
