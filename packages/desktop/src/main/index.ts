import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { openLedger } from '@ratchet/core';
import { createConnectionTester } from '@ratchet/runtime';
import { app, BrowserWindow, dialog, ipcMain, powerSaveBlocker, safeStorage, shell } from 'electron';
import { IPC } from '../shared/channels.js';
import { createEngineForApp } from './engine.js';
import { createGithubApi } from './github.js';
import { HarnessCache } from './harnesses.js';
import { isTrustedUrl, registerIpc } from './ipc.js';
import { defaultLivePaths } from './live-config.js';
import { createPowerSaveHook } from './power.js';
import { createService } from './service.js';
import { SettingsStore } from './settings.js';

// Test/CI override: keep the ledger DB and settings in an isolated directory (used by packages/e2e).
// Must run before the app is ready, as userData is read from it.
if (process.env['RATCHET_USER_DATA_DIR']) app.setPath('userData', process.env['RATCHET_USER_DATA_DIR']);

let mainWindow: BrowserWindow | null = null;

function createWindow(): void {
  const win = new BrowserWindow({
    width: 1360,
    height: 880,
    minWidth: 1100,
    minHeight: 700,
    backgroundColor: '#0B1019',
    titleBarStyle: process.platform === 'darwin' ? 'hiddenInset' : 'default',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  mainWindow = win;
  win.on('closed', () => {
    if (mainWindow === win) mainWindow = null;
  });

  // Never navigate the app window away, and open external links in the system browser.
  win.webContents.on('will-navigate', (e, url) => {
    if (url !== win.webContents.getURL()) e.preventDefault();
  });
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https:\/\//.test(url)) void shell.openExternal(url);
    return { action: 'deny' };
  });

  if (process.env['ELECTRON_RENDERER_URL']) {
    void win.loadURL(process.env['ELECTRON_RENDERER_URL']);
  } else {
    void win.loadFile(join(__dirname, '../renderer/index.html'));
  }
}

/** Drizzle migrations: shipped next to the app when packaged (extraResources), else read from the core sources. */
function migrationsFolder(): string {
  return app.isPackaged ? join(process.resourcesPath, 'migrations') : join(__dirname, '../../../core/src/ledger/migrations');
}

app.whenReady().then(() => {
  const userData = app.getPath('userData');
  mkdirSync(userData, { recursive: true });
  const ledger = openLedger(join(userData, 'ratchet.db'), { migrationsFolder: migrationsFolder() });
  const settings = new SettingsStore(join(userData, 'settings.json'), {
    isAvailable: () => safeStorage.isEncryptionAvailable(),
    encrypt: (s) => safeStorage.encryptString(s),
    decrypt: (b) => safeStorage.decryptString(b),
  });

  // The bundled OpenTofu module (packaged: Resources/infra/aws) and, per sandbox, state under userData. Hidden from the UI.
  const liveDefaults = defaultLivePaths({
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    repoRoot: resolve(process.env['RATCHET_REPO_ROOT'] ?? join(__dirname, '../../../..')),
  });
  // Agent CLIs on this machine: probed in the main process on launch, again on "Re-detect".
  const harnesses = new HarnessCache();
  void harnesses.get();
  // RATCHET_GITHUB_CLIENT_ID: the OAuth app for "Sign in with GitHub" (device flow). RATCHET_GITHUB_API_URL / _WEB_URL: GitHub
  // Enterprise, and the fake GitHub the end-to-end tests run against.
  const github = createGithubApi({
    clientId: process.env['RATCHET_GITHUB_CLIENT_ID'],
    ...(process.env['RATCHET_GITHUB_API_URL'] ? { apiUrl: process.env['RATCHET_GITHUB_API_URL'] } : {}),
    ...(process.env['RATCHET_GITHUB_WEB_URL'] ? { webUrl: process.env['RATCHET_GITHUB_WEB_URL'] } : {}),
  });
  const stateRoot = join(userData, 'sandboxes');

  const service = createService({
    ledger,
    settings,
    liveDefaults,
    harnesses,
    github,
    openExternal: (url) => {
      if (/^https:\/\//.test(url)) void shell.openExternal(url);
    },
    connectionTester: createConnectionTester(),
    // Test/CI override: answer the native folder dialog from the environment.
    pickFolder: async () => {
      const preset = process.env['RATCHET_E2E_PICK_FOLDER'];
      if (preset !== undefined) return preset || null;
      const res = await dialog.showOpenDialog({ properties: ['openDirectory'], title: 'Choose a local repository' });
      return res.canceled || !res.filePaths[0] ? null : res.filePaths[0];
    },
    createApp: ({ configFor, connectors }) =>
      createEngineForApp(ledger, {
        live: { settings, connectors, stateRoot, defaults: liveDefaults, harnesses, appVersion: app.getVersion(), configFor },
      }),
    saveReport: async (defaultName, markdown) => {
      // Test/CI override: write the report into this directory instead of asking through a native dialog.
      const autoDir = process.env['RATCHET_E2E_SAVE_DIR'];
      if (autoDir) {
        mkdirSync(autoDir, { recursive: true });
        const file = join(autoDir, defaultName);
        writeFileSync(file, markdown, 'utf8');
        return file;
      }
      const res = await dialog.showSaveDialog({ defaultPath: defaultName, filters: [{ name: 'Markdown', extensions: ['md'] }] });
      if (res.canceled || !res.filePath) return null;
      writeFileSync(res.filePath, markdown, 'utf8');
      return res.filePath;
    },
  });

  registerIpc(ipcMain, service, {
    isTrustedSender: (event) => {
      const url = (event as { senderFrame?: { url?: string } }).senderFrame?.url ?? '';
      return isTrustedUrl(url, process.env['ELECTRON_RENDERER_URL']);
    },
  });
  // Keep the machine awake while a run is active (agents run on this machine and the control loop must keep ticking).
  const power = createPowerSaveHook(powerSaveBlocker);
  service.onEvent((e) => {
    power.onEvent(e);
    mainWindow?.webContents.send(IPC.event, e);
  });

  app.on('before-quit', () => {
    power.release();
    service.dispose();
    ledger.close();
  });

  createWindow();
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
