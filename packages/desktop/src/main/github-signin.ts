import type { GithubFlowVM } from '../shared/types.js';
import type { GithubApi } from './github.js';

export interface SignInDeps {
  api: GithubApi;
  /** Keep the token of the signed-in account (the connector store: keychain for the token, the login as metadata). */
  save: (token: string, login: string, connectorId: string | undefined) => void;
  /** Open the verification page in the system browser. */
  openExternal?: (url: string) => void;
  /** Injectable for tests (default: a real timer that `cancel` can interrupt). */
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
  now?: () => number;
  /** Called after the token is stored (the service rebuilds what depends on it). */
  onSignedIn?: () => void;
}

const defaultSleep = (ms: number, signal: AbortSignal): Promise<void> =>
  new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    signal.addEventListener('abort', () => (clearTimeout(t), resolve()), { once: true });
  });

/**
 * "Sign in with GitHub": the OAuth device flow. `start` returns the code to show and keeps polling in the background;
 * on success the token goes to the keychain-backed connector store (never to the renderer) and the account login is kept.
 * `start({ connectorId })` re-signs-in an existing connector.
 */
export class GithubSignIn {
  private flow: GithubFlowVM | null = null;
  private ac: AbortController | null = null;

  constructor(private readonly d: SignInDeps) {}

  current(): GithubFlowVM | null {
    return this.flow;
  }

  async start(opts: { connectorId?: string } = {}): Promise<GithubFlowVM> {
    this.cancel();
    const now = this.d.now ?? Date.now;
    const code = await this.d.api.deviceStart();
    const flow: GithubFlowVM = {
      state: 'pending',
      userCode: code.userCode,
      verificationUri: code.verificationUri,
      expiresAt: now() + code.expiresIn * 1000,
      message: null,
    };
    this.flow = flow;
    const ac = new AbortController();
    this.ac = ac;
    try {
      this.d.openExternal?.(code.verificationUri);
    } catch {
      /* the code is shown in the app anyway */
    }
    void this.poll(code.deviceCode, code.interval, flow, ac, opts.connectorId);
    return flow;
  }

  cancel(): void {
    this.ac?.abort();
    this.ac = null;
    this.flow = null;
  }

  private async poll(deviceCode: string, startInterval: number, flow: GithubFlowVM, ac: AbortController, connectorId: string | undefined): Promise<void> {
    const sleep = this.d.sleep ?? defaultSleep;
    const now = this.d.now ?? Date.now;
    let interval = Math.max(1, startInterval);
    const end = (state: GithubFlowVM['state'], message: string | null): void => {
      if (this.flow === flow) this.flow = { ...flow, state, message };
    };
    while (!ac.signal.aborted) {
      await sleep(interval * 1000, ac.signal);
      if (ac.signal.aborted || this.flow !== flow) return;
      if (now() >= flow.expiresAt) return end('expired', 'The code expired. Start again.');
      let r;
      try {
        r = await this.d.api.devicePoll(deviceCode);
      } catch (err) {
        return end('error', (err as Error).message);
      }
      switch (r.status) {
        case 'pending':
          continue;
        case 'slow_down':
          interval = Math.max(interval + 5, r.interval);
          continue;
        case 'expired':
          return end('expired', 'The code expired. Start again.');
        case 'denied':
          return end('denied', 'Access was denied on GitHub.');
        case 'error':
          return end('error', r.message);
        case 'ok': {
          try {
            const { login } = await this.d.api.user(r.token);
            this.d.save(r.token, login, connectorId);
          } catch (err) {
            return end('error', (err as Error).message);
          }
          if (this.flow === flow) this.flow = null;
          this.d.onSignedIn?.();
          return;
        }
      }
    }
  }
}
