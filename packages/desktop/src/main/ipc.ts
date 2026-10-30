import type { z } from 'zod';
import { IPC, type InvokeMethod } from '../shared/channels.js';
import { ARGS } from '../shared/ipc.js';
import type { RatchetService } from './service.js';

/** The slice of Electron's `ipcMain` we use (lets tests register handlers without Electron). */
export interface IpcMainLike {
  handle(channel: string, listener: (event: unknown, ...args: unknown[]) => unknown): void;
}

export interface RegisterOptions {
  /** Reject calls from frames that are not the app's own renderer. Default: accept (tests). */
  isTrustedSender?: (event: unknown) => boolean;
}

export class IpcValidationError extends Error {
  constructor(method: string, issues: string) {
    super(`Invalid arguments for ${method}: ${issues}`);
    this.name = 'IpcValidationError';
  }
}

const METHODS = Object.keys(ARGS) as InvokeMethod[];

/** `file://` pages and the electron-vite dev server origin are the only trusted renderers. */
export function isTrustedUrl(url: string, devServerUrl?: string): boolean {
  if (url.startsWith('file://')) return true;
  if (!devServerUrl) return false;
  try {
    return new URL(url).origin === new URL(devServerUrl).origin;
  } catch {
    return false;
  }
}

/**
 * Register one handler per `RatchetApi` method. Every call's arguments are parsed with the zod tuple in
 * `ARGS` before the service runs; anything that does not parse is rejected and nothing executes.
 */
export function registerIpc(ipc: IpcMainLike, service: RatchetService, opts: RegisterOptions = {}): void {
  for (const method of METHODS) {
    const schema = ARGS[method] as z.ZodType<unknown[]>;
    ipc.handle(IPC[method], async (event, ...args) => {
      if (opts.isTrustedSender && !opts.isTrustedSender(event)) throw new Error(`Untrusted sender for ${method}`);
      const parsed = schema.safeParse(args);
      if (!parsed.success) {
        throw new IpcValidationError(method, parsed.error.issues.map((i) => `${i.path.join('.') || 'args'}: ${i.message}`).join('; '));
      }
      const fn = service[method] as (...a: unknown[]) => Promise<unknown>;
      return fn.apply(service, parsed.data);
    });
  }
}
