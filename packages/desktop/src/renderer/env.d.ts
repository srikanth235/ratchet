import type { RatchetApi } from '../shared/types.js';

declare global {
  interface Window {
    ratchet: RatchetApi;
  }
}
