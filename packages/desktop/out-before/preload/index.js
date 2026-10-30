"use strict";
const electron = require("electron");
const IPC = {
  listOptimizations: "ratchet:listOptimizations",
  getOptimization: "ratchet:getOptimization",
  ratchetHistory: "ratchet:ratchetHistory",
  candidatesWithResults: "ratchet:candidatesWithResults",
  spendBreakdown: "ratchet:spendBreakdown",
  recentActivity: "ratchet:recentActivity",
  agentEvents: "ratchet:agentEvents",
  validateConfig: "ratchet:validateConfig",
  preflight: "ratchet:preflight",
  sandboxInfo: "ratchet:sandboxInfo",
  getSettings: "ratchet:getSettings",
  createOptimization: "ratchet:createOptimization",
  startOptimization: "ratchet:startOptimization",
  stopOptimization: "ratchet:stopOptimization",
  pauseAgent: "ratchet:pauseAgent",
  resumeAgent: "ratchet:resumeAgent",
  messageAgent: "ratchet:messageAgent",
  openPullRequest: "ratchet:openPullRequest",
  teardown: "ratchet:teardown",
  exportReport: "ratchet:exportReport",
  setMode: "ratchet:setMode",
  setAwsProfile: "ratchet:setAwsProfile",
  setSecret: "ratchet:setSecret",
  clearSecret: "ratchet:clearSecret",
  event: "ratchet:event"
};
const call = (method) => (...args) => electron.ipcRenderer.invoke(IPC[method], ...args);
const methods = [
  "listOptimizations",
  "getOptimization",
  "ratchetHistory",
  "candidatesWithResults",
  "spendBreakdown",
  "recentActivity",
  "agentEvents",
  "validateConfig",
  "preflight",
  "sandboxInfo",
  "getSettings",
  "createOptimization",
  "startOptimization",
  "stopOptimization",
  "pauseAgent",
  "resumeAgent",
  "messageAgent",
  "openPullRequest",
  "teardown",
  "exportReport",
  "setMode",
  "setAwsProfile",
  "setSecret",
  "clearSecret"
];
const api = {
  ...Object.fromEntries(methods.map((m) => [m, call(m)])),
  onEvent(listener) {
    const handler = (_, e) => listener(e);
    electron.ipcRenderer.on(IPC.event, handler);
    return () => {
      electron.ipcRenderer.removeListener(IPC.event, handler);
    };
  }
};
electron.contextBridge.exposeInMainWorld("ratchet", api);
