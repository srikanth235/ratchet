import { contextBridge, ipcRenderer } from 'electron';
import { IPC } from '../shared/channels.js';
import type { EngineEventVM, RatchetApi } from '../shared/types.js';

type Invoke = Exclude<keyof RatchetApi, 'onEvent'>;

const call =
  (method: Invoke) =>
  (...args: unknown[]) =>
    ipcRenderer.invoke(IPC[method], ...args);

const methods: Invoke[] = [
  'listOptimizations', 'getOptimization', 'ratchetHistory', 'candidatesWithResults', 'spendBreakdown',
  'recentActivity', 'agentEvents', 'validateConfig', 'preflight', 'sandboxInfo', 'getSettings',
  'planSandbox', 'applySandbox', 'destroySandbox',
  'createOptimization', 'startOptimization', 'stopOptimization', 'pauseAgent', 'resumeAgent',
  'messageAgent', 'openPullRequest', 'teardown', 'exportReport', 'setMode', 'setAwsProfile',
  'setSecret', 'clearSecret', 'saveLiveSettings', 'testConnection',
];

const api = {
  ...Object.fromEntries(methods.map((m) => [m, call(m)])),
  onEvent(listener: (e: EngineEventVM) => void): () => void {
    const handler = (_: unknown, e: EngineEventVM) => listener(e);
    ipcRenderer.on(IPC.event, handler);
    return () => {
      ipcRenderer.removeListener(IPC.event, handler);
    };
  },
} as RatchetApi;

contextBridge.exposeInMainWorld('ratchet', api);
