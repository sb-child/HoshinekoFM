import { ipcMain, BrowserWindow, dialog, nativeImage } from 'electron';
import path from 'path';
import { promises as fs, writeFileSync, existsSync } from 'fs';
import { getCachedDragIconPath } from '../fsUtils';

export function registerWindowHandlers(mainWindowRef: () => BrowserWindow | null) {
  ipcMain.handle('dialog:open-file', async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      properties: ['openFile'],
      filters: [{ name: 'CSS Files', extensions: ['css'] }]
    });
    if (canceled) return null;
    return filePaths[0];
  });

  ipcMain.on('cache:drag-icon', (_event, iconName: string, pngBase64: string) => {
    const target = getCachedDragIconPath(iconName);
    try {
      const buf = Buffer.from(pngBase64, 'base64');
      writeFileSync(target, buf);
    } catch {
      // Silently skip — fallback will be used
    }
  });

  ipcMain.on('dnd:start', (event, filePaths: string | string[]) => {
    const files = Array.isArray(filePaths) ? filePaths : [filePaths];
    const cachedPath = getCachedDragIconPath('insert_drive_file');
    if (!existsSync(cachedPath)) {
      try {
        const img = nativeImage.createFromDataURL(
          'data:image/svg+xml;base64,' +
          Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96"><rect width="96" height="96" rx="16" fill="#4285F4"/></svg>').toString('base64')
        );
        writeFileSync(cachedPath, img.toPNG());
      } catch { /* continue */ }
    }
    event.sender.startDrag({
      file: files[0],
      files,
      icon: cachedPath,
    });
  });

  ipcMain.handle('window:set-icon', async (_, iconType: 'light' | 'dark' | string) => {
    const mainWindow = mainWindowRef();
    if (!mainWindow) return;

    if (path.isAbsolute(iconType)) {
      mainWindow.setIcon(iconType);
    } else {
      const possiblePaths = [
        path.join(__dirname, `../assets/icon-${iconType}.png`),
        path.join(process.resourcesPath, `assets/icon-${iconType}.png`)
      ];
      for (const p of possiblePaths) {
        try {
          await fs.access(p);
          mainWindow.setIcon(p);
          return;
        } catch { /* continue */ }
      }
    }
  });

  ipcMain.handle('app:get-startup-path', async () => {
    const args = process.argv;
    const lastArg = args[args.length - 1];

    if (!lastArg || lastArg === '.' || lastArg === process.execPath) {
      return null;
    }

    try {
      if (path.isAbsolute(lastArg)) {
        const stats = await fs.stat(lastArg);
        if (stats.isDirectory()) {
          return lastArg;
        } else if (stats.isFile()) {
          return path.dirname(lastArg);
        }
      }
    } catch {
      return null;
    }
    return null;
  });
}
