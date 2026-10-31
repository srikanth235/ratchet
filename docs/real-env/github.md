# Real-environment testing: GitHub (`@ratchet/github`)

> **Written for the TypeScript engine.** Names such as `@ratchet/...`, `packages/...`, `createLiveRuntime` and the `.mjs`/`node` snippets refer to
> the code at commit `fc503b1`; the Rust replacement is `crates/ratchet-github` (see [CUTOVER.md](../CUTOVER.md)). What to verify on a real account, and why,
> still applies; re-express the snippets with `ratchetd` / `ratchet` before using them.

`GitHubGitHost` is the real `GitHost`; `LocalGitHost` is its simulation twin (a
local bare repo). Both share one implementation of the git logic
(`BaseGitHost`), so what the local tests prove about trailer parsing, diffs,
checkout and squashing carries over. What differs is transport and the PR call.

## What is mocked in CI

| Piece | In `pnpm test` | Needs a real run |
|---|---|---|
| git operations (log, diff, read-tree, commit-tree) | real `git` against temp repos | – |
| `fetch` / `push` to GitHub | the `https://github.com/...` URL is rewritten to a local bare repo by a recording runner | HTTPS auth, branch protection, rate limits |
| auth header (`GIT_CONFIG_*` extraheader) | asserted present in env, absent from argv | GitHub accepting it |
| Octokit `pulls.create/list/update` | fake object | real API, permissions, 422 semantics |
| token redaction | unit-tested on the default runner | – |

## Credentials

Use a **repo-scoped** credential for the single target repo; never a classic PAT.

**Fine-grained personal access token** (quickest):

- Resource owner: the org/user owning the repo; *Only select repositories* → the target repo.
- Repository permissions: **Contents: Read and write** (fetch, push `ratchet/*` branches),
  **Pull requests: Read and write** (open/update the PR). Metadata: Read (implicit).
- Nothing else (no Actions, Administration, Workflows).

**GitHub App installation token** (preferred for a team): install the app on the
one repo with the same two permissions and pass the short-lived installation
token as `token`. Mint it outside Ratchet (e.g. `actions/create-github-app-token`
or the `POST /app/installations/{id}/access_tokens` API) and refresh it before it
expires (1 hour).

If branch protection forbids pushes to `ratchet/**`, exempt that pattern; the
host force-pushes `ratchet/<opt>/result` (a branch Ratchet owns) so a re-export
after more wins updates it. It never touches the base branch.

## How the token is handled

- Never in argv or the remote URL (`https://github.com/<owner>/<repo>.git`).
- For network git calls only, it is passed via the environment:
  `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=http.https://github.com/.extraheader`,
  `GIT_CONFIG_VALUE_0=AUTHORIZATION: basic <base64 x-access-token:TOKEN>`.
  Requires git ≥ 2.31.
- The default runner scrubs the token and its base64 form from error messages.
- Octokit gets it as `auth` (Authorization header).
- Reads use a local bare mirror (`cacheDir`, default a temp dir) holding
  `refs/remotes/origin/*`; it contains no credentials.

## Running it against a real repo

Use a throwaway repo (e.g. a fork of `examples/demo-search`).

```ts
import { GitHubGitHost, buildResultReport, buildSquashPlanFromLedger } from '@ratchet/github';

const host = new GitHubGitHost({ repo: 'you/demo-search-sandbox', token: process.env.RATCHET_GH_TOKEN! });

// 1. branch creation
await host.createBranch('main', 'ratchet/test/claude');
// 2. push a commit with a `Ratchet-Candidate: true` trailer to that branch by hand, then:
const found = await host.listCandidates('ratchet/test/claude');   // expect 1 commit
await host.changedFiles('main', found[0].sha);
await host.checkout(found[0].sha, '/tmp/ratchet-co');
// 3. PR from a ledger with verified wins
const plan = buildSquashPlanFromLedger(ledger, optId);
const report = buildResultReport(ledger, optId);
await host.openPullRequest({
  base: 'main', head: `ratchet/${optId}/result`,
  title: plan.title, body: report.markdown, shas: plan.shas, commitMessage: plan.message,
});
```

Checklist:

1. `createBranch` creates the branch; a second call for the same name fails (no overwrite).
2. `listCandidates` returns only trailer commits, oldest first; `sinceSha` excludes older ones.
3. The PR opens with the report as its body; the single squashed commit has the
   `Co-authored-by` trailers and the **same tree** as the last verified SHA
   (`git rev-parse <result>^{tree}` equals `git rev-parse <final>^{tree}`).
4. Re-running `openPullRequest` for the same head updates the existing PR (422 path) rather than failing.
5. With `GIT_TRACE=1 GIT_CURL_VERBOSE=1` the token appears only in the (masked) `Authorization` header, never in a URL or process list (`ps`).
6. Revoke the token and confirm errors surface without echoing it.

## Known gaps

- Rate limits / retries on GitHub 5xx are not handled; a failed call throws.
- The squash commit is parented at `merge-base(base, finalSha)`, so a base that
  moved on during the run produces a PR GitHub may report as conflicting; resolve by rebasing the result branch.
- The squash assumes later wins were rebased onto earlier ones (the runner does this); wins from a branch
  that was not rebased are not merged in.
- GitHub Enterprise: `host` / `apiUrl` options exist but are untested.
