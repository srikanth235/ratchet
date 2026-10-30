import type { RatchetApi } from './types.js';

// Kept free of zod so the sandboxed preload script can import it.

/** IPC channel per `RatchetApi` method (everything except `onEvent`), plus the event push channel. */
export const IPC = {
  listOptimizations: 'ratchet:listOptimizations',
  getOptimization: 'ratchet:getOptimization',
  ratchetHistory: 'ratchet:ratchetHistory',
  candidatesWithResults: 'ratchet:candidatesWithResults',
  spendBreakdown: 'ratchet:spendBreakdown',
  recentActivity: 'ratchet:recentActivity',
  agentEvents: 'ratchet:agentEvents',
  validateConfig: 'ratchet:validateConfig',
  preflight: 'ratchet:preflight',
  sandboxInfo: 'ratchet:sandboxInfo',
  getSettings: 'ratchet:getSettings',
  planSandbox: 'ratchet:planSandbox',
  applySandbox: 'ratchet:applySandbox',
  destroySandbox: 'ratchet:destroySandbox',
  createOptimization: 'ratchet:createOptimization',
  startOptimization: 'ratchet:startOptimization',
  stopOptimization: 'ratchet:stopOptimization',
  pauseAgent: 'ratchet:pauseAgent',
  resumeAgent: 'ratchet:resumeAgent',
  messageAgent: 'ratchet:messageAgent',
  openPullRequest: 'ratchet:openPullRequest',
  teardown: 'ratchet:teardown',
  exportReport: 'ratchet:exportReport',
  setMode: 'ratchet:setMode',
  setAwsProfile: 'ratchet:setAwsProfile',
  setSecret: 'ratchet:setSecret',
  clearSecret: 'ratchet:clearSecret',
  saveLiveSettings: 'ratchet:saveLiveSettings',
  testConnection: 'ratchet:testConnection',
  event: 'ratchet:event',
} as const satisfies Record<keyof Omit<RatchetApi, 'onEvent'> | 'event', string>;

export type InvokeMethod = Exclude<keyof typeof IPC, 'event'>;

