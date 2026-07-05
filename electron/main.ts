import { app, BrowserWindow, protocol, net, ipcMain } from 'electron';
import path from 'path';
import url from 'url';
import os from 'os';
import { promises as fs } from 'fs';
import { spawn } from 'child_process';
import { setupPtyHandlers, setMainWindow } from './pty';
import { getThumbnail } from './fsUtils';
import { startWatching, stopWatching, stopAllWatching } from './fsWatcher';
import { registerFsHandlers } from './handlers/fs';
import { registerSystemHandlers, setupUdisks2Monitor } from './handlers/system';
import { registerWindowHandlers } from './handlers/window';
import { initJobHandlers } from './jobs';

let mainWindow: BrowserWindow | null = null;

function getMainWindow(): BrowserWindow | null {
  return mainWindow;
}

// Initialize PTY handlers
setupPtyHandlers();
let terminalProcess: ReturnType<typeof spawn> | null = null;

// Register protocol before app ready
protocol.registerSchemesAsPrivileged([
  { scheme: 'media', privileges: { secure: true, supportFetchAPI: true, bypassCSP: true } }
]);

// Wayland & GPU Flags
app.commandLine.appendSwitch('enable-features', 'WaylandWindowDecorations');
app.commandLine.appendSwitch('ozone-platform-hint', 'auto');

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1200,
    height: 800,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: false
    },
    autoHideMenuBar: true,
  });
  setMainWindow(mainWindow);

  if (process.env.NODE_ENV === 'development') {
    mainWindow.loadURL('http://localhost:5173');
    mainWindow.webContents.openDevTools();
  } else {
    mainWindow.loadFile(path.join(__dirname, '../dist/index.html'));
  }
}

ipcMain.handle('theme:get-css', async () => {
  const homeDir = os.homedir();
  const themePath = path.join(homeDir, '.config/matugen/theme.css');
  try {
    return await fs.readFile(themePath, 'utf-8');
  } catch {
    return null;
  }
});

ipcMain.handle('fs:watch-dir', (_event, dir: string) => {
  try {
    startWatching(dir, (changedDir) => {
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('fs:dir-changed', changedDir);
      }
    });
  } catch {
    // directory already gone or inaccessible — silently skip
  }
});

ipcMain.handle('fs:unwatch-dir', (_event, dir: string) => {
  stopWatching(dir);
});

registerFsHandlers();
registerSystemHandlers(getMainWindow);
registerWindowHandlers(getMainWindow);
initJobHandlers();

app.whenReady().then(() => {
  protocol.handle('media', async (request) => {
    const filePath = request.url.slice('media://'.length);
    const decodedPath = decodeURIComponent(filePath);

    const thumbPath = await getThumbnail(decodedPath, 256);
    if (thumbPath) {
      return net.fetch(url.pathToFileURL(thumbPath).toString());
    }

    return net.fetch(url.pathToFileURL(decodedPath).toString());
  });
  createWindow();
  setupUdisks2Monitor(mainWindow);
});

app.on('window-all-closed', () => {
  if (terminalProcess) {
    if (terminalProcess.pid) process.kill(terminalProcess.pid);
    terminalProcess = null;
  }
  stopAllWatching();
  if (process.platform !== 'darwin') {
    app.quit();
  }
});

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) {
    createWindow();
  }
});
