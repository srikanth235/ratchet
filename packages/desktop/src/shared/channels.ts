import type { RatchetApi } from './types.js';

// Kept free of zod so the sandboxed preload script can import it.

/** IPC channel per `RatchetApi` method (everything except `onEvent`), plus the event push channel. */
export const IPC = {
  listOptimizations: 'ratchet:listOptimizations',
  getOptimization: 'ratchet:getOptimization',
  ratchetHistory: 'ratchet:ratchetHistory',
  winningChange: 'ratchet:winningChange',
  candidatesWithResults: 'ratchet:candidatesWithResults',
  spendBreakdown: 'ratchet:spendBreakdown',
  recentActivity: 'ratchet:recentActivity',
  agentEvents: 'ratchet:agentEvents',
  validateConfig: 'ratchet:validateConfig',
  preflight: 'ratchet:preflight',
  getSettings: 'ratchet:getSettings',
  environments: 'ratchet:environments',
  githubStatus: 'ratchet:githubStatus',
  githubSignIn: 'ratchet:githubSignIn',
  githubSignOut: 'ratchet:githubSignOut',
  listRepos: 'ratchet:listRepos',
  inspectRepo: 'ratchet:inspectRepo',
  pickLocalFolder: 'ratchet:pickLocalFolder',
  awsProfiles: 'ratchet:awsProfiles',
  detectHarnesses: 'ratchet:detectHarnesses',
  redetectHarnesses: 'ratchet:redetectHarnesses',
  listTemplates: 'ratchet:listTemplates',
  proposeTemplate: 'ratchet:proposeTemplate',
  createOptimization: 'ratchet:createOptimization',
  startOptimization: 'ratchet:startOptimization',
  stopOptimization: 'ratchet:stopOptimization',
  pauseAgent: 'ratchet:pauseAgent',
  resumeAgent: 'ratchet:resumeAgent',
  messageAgent: 'ratchet:messageAgent',
  openPullRequest: 'ratchet:openPullRequest',
  exportReport: 'ratchet:exportReport',
  endSandbox: 'ratchet:endSandbox',
  setAwsProfile: 'ratchet:setAwsProfile',
  setSecret: 'ratchet:setSecret',
  clearSecret: 'ratchet:clearSecret',
  saveLiveSettings: 'ratchet:saveLiveSettings',
  testConnection: 'ratchet:testConnection',
  event: 'ratchet:event',
} as const satisfies Record<keyof Omit<RatchetApi, 'onEvent'> | 'event', string>;

export type InvokeMethod = Exclude<keyof typeof IPC, 'event'>;

