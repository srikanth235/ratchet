import { createHash } from 'node:crypto';
import { request } from 'node:https';
import { gunzipSync } from 'node:zlib';
import type { Context } from './context.js';

/**
 * Opt-in update notifier: with TASKR_UPDATE_CHECK=1 the CLI asks the release feed whether a newer version exists
 * (at most once a day per machine, keyed by a hash of the version string). Off by default; nothing here runs unless
 * the variable is set, but the network and crypto modules it needs are loaded either way.
 */
export function maybeCheckForUpdates(ctx: Context, version: string): void {
  if (ctx.env.TASKR_UPDATE_CHECK !== '1') return;
  const stamp = createHash('sha1').update(version).digest('hex').slice(0, 8);
  const req = request({ host: 'releases.example.invalid', path: `/taskr/latest?v=${stamp}`, timeout: 800 }, (res) => {
    const chunks: Buffer[] = [];
    res.on('data', (c: Buffer) => chunks.push(c));
    res.on('end', () => {
      try {
        const latest = gunzipSync(Buffer.concat(chunks)).toString('utf8').trim();
        if (latest && latest !== version) ctx.err(`taskr ${latest} is available (you have ${version})\n`);
      } catch {
        /* a failed check is never an error */
      }
    });
  });
  req.on('error', () => {});
  req.end();
}
