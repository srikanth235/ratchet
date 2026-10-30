import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { openLedger } from '@ratchet/core';
import { app, BrowserWindow, dialog, ipcMain, safeStorage, shell } from 'electron';
import { IPC } from '../shared/channels.js';
import { createEngineForApp } from './engine.js';
import { isTrustedUrl, registerIpc } from './ipc.js';
import { createService } from './service.js';
import { SettingsStore } from './settings.js';

// Test/CI override: keep the ledger DB and settings in an isolated directory (used by packages/e2e).
// Must run before the app is ready, as userData is read from it.
if (process.env['RATCHET_USER_DATA_DIR']) app.setPath('userData', process.env['RATCHET_USER_DATA_DIR']);

// Test/CI override: the simulation runtime spawns `process.execPath` to run plain Node scripts; inside Electron that is
// the Electron binary, so point it at a real Node (packages/e2e sets this). Not needed once the runtime spawns with
// ELECTRON_RUN_AS_NODE=1.
if (process.env['RATCHET_NODE_BIN']) process.execPath = process.env['RATCHET_NODE_BIN'];

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

  const service = createService({
    ledger,
    settings,
    createApp: (mode) => createEngineForApp(ledger, mode),
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
  service.onEvent((e) => mainWindow?.webContents.send(IPC.event, e));

  app.on('before-quit', () => {
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
