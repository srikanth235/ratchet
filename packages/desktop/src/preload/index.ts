import { contextBridge, ipcRenderer } from 'electron';
import { IPC, type RatchetApi } from '../shared/types.js';

const api: RatchetApi = {
  listOptimizations: () => ipcRenderer.invoke(IPC.listOptimizations),
};

contextBridge.exposeInMainWorld('ratchet', api);
