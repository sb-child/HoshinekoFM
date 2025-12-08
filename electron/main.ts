import { app, BrowserWindow, ipcMain, protocol, net, shell, dialog } from 'electron';
import path from 'path';
import { promises as fs } from 'fs';
import url from 'url';
import os from 'os';
import { spawn, exec, execFile } from 'child_process';
import { promisify } from 'util';
import dbus from 'dbus-next';

const execAsync = promisify(exec);
const execFileAsync = promisify(execFile);

// --- State ---
let mainWindow: BrowserWindow | null = null;
let terminalProcess: any = null;
let terminalSessionProxy: any = null;

// --- Helper Functions ---
async function setupTerminal(pid: number) {
    const bus = dbus.sessionBus();
    const serviceName = `org.kde.konsole - ${pid} `;

    for (let i = 0; i < 20; i++) {
        try {
            // Check if service exists by listing names? No, just try to get object.
            const sessionPath = '/Sessions/1';
            const sessionObj = await bus.getProxyObject(serviceName, sessionPath);
            const sessionInterface = sessionObj.getInterface('org.kde.konsole.Session');

            if (sessionInterface) {
                terminalSessionProxy = sessionInterface;
                console.log('Connected to Konsole DBus:', serviceName);
                return;
            }
        } catch (e) {
            // Ignore errors while waiting
        }
        await new Promise(r => setTimeout(r, 500));
    }
    console.warn('Failed to connect to Konsole DBus after retries');
}

// Register protocol before app ready
protocol.registerSchemesAsPrivileged([
    { scheme: 'media', privileges: { secure: true, supportFetchAPI: true, bypassCSP: true } }
]);

// Wayland & GPU Flags
// Suppress "Incomplete image description" warning on Wayland
app.commandLine.appendSwitch('enable-features', 'WaylandWindowDecorations');
app.commandLine.appendSwitch('ozone-platform-hint', 'auto');
// Optional: Disable GPU rasterization if glitches persist, but 'auto' ozone hint often fixes it.
// app.commandLine.appendSwitch('disable-gpu-compositing');

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

    if (process.env.NODE_ENV === 'development') {
        mainWindow.loadURL('http://localhost:5173');
        mainWindow.webContents.openDevTools();
    } else {
        mainWindow.loadFile(path.join(__dirname, '../dist/index.html'));
    }
}

// --- IPC Handlers ---

ipcMain.handle('theme:get-css', async () => {
    const homeDir = os.homedir();
    const themePath = path.join(homeDir, '.config/matugen/theme.css');
    try {
        const css = await fs.readFile(themePath, 'utf-8');
        return css;
    } catch (error) {
        return null;
    }
});

ipcMain.handle('fs:list-dir', async (_, dirPath: string) => {
    try {
        const targetPath = dirPath || os.homedir();
        const entries = await fs.readdir(targetPath, { withFileTypes: true });
        const results = await Promise.all(entries.map(async (entry) => {
            try {
                const stats = await fs.stat(path.join(targetPath, entry.name));
                return {
                    name: entry.name,
                    path: path.join(targetPath, entry.name),
                    isDirectory: entry.isDirectory(),
                    size: stats.size,
                    mtime: stats.mtime
                };
            } catch (e) {
                return null;
            }
        }));
        return results.filter(r => r !== null);
    } catch (error) {
        console.error('Failed to list dir', dirPath, error);
        throw error;
    }
});

ipcMain.handle('fs:get-places', () => {
    const home = app.getPath('home');
    return [
        { name: 'Home', path: home, icon: '🏠' },
        { name: 'Desktop', path: app.getPath('desktop'), icon: '🖥️' },
        { name: 'Documents', path: app.getPath('documents'), icon: '📄' },
        { name: 'Downloads', path: app.getPath('downloads'), icon: '⬇️' },
        { name: 'Music', path: app.getPath('music'), icon: '🎵' },
        { name: 'Pictures', path: app.getPath('pictures'), icon: '🖼️' },
        { name: 'Videos', path: app.getPath('videos'), icon: '🎥' },
    ];
});

ipcMain.handle('fs:copy', async (_, source: string, dest: string) => {
    // Simple copy for now. For directories, we might need recursive copy.
    // fs.cp is available in Node 16.7.0+
    await fs.cp(source, dest, { recursive: true, force: false });
    return true;
});

ipcMain.handle('fs:move', async (_, source: string, dest: string) => {
    await fs.rename(source, dest);
    return true;
});

ipcMain.handle('fs:trash', async (_, filePath: string) => {
    await shell.trashItem(filePath);
    return true;
});

ipcMain.handle('fs:rename', async (_, oldPath: string, newPath: string) => {
    await fs.rename(oldPath, newPath);
    return true;
});

ipcMain.handle('fs:mkdir', async (_, dirPath: string) => {
    await fs.mkdir(dirPath, { recursive: true });
    return true;
});

ipcMain.handle('fs:open', async (_, filePath: string) => {
    const error = await shell.openPath(filePath);
    return error; // returns error string or empty if success
});

ipcMain.handle('fs:extract', async (_, filePath: string) => {
    try {
        const dir = path.dirname(filePath);
        const ext = path.extname(filePath).toLowerCase();

        // Simple extraction logic - extend as needed
        if (ext === '.zip') {
            await execAsync(`unzip "${filePath}" -d "${dir}"`);
        } else if (ext === '.tar' || ext === '.gz' || ext === '.xz') {
            await execAsync(`tar -xf "${filePath}" -C "${dir}"`);
        } else {
            return false; // Unsupported
        }
        return true;
    } catch (e) {
        console.error('Extract failed', e);
        return false;
    }
});

// Cache for apps
let appsCache: { name: string; icon: string; exec: string; }[] | null = null;

ipcMain.handle('system:get-apps', async () => {
    if (appsCache) return appsCache;

    const apps: { name: string; icon: string; exec: string; }[] = [];
    const dirs = ['/usr/share/applications', '/usr/local/share/applications', path.join(os.homedir(), '.local/share/applications')];

    for (const dir of dirs) {
        try {
            const files = await fs.readdir(dir); // This acts on directory path
            for (const file of files) {
                if (file.endsWith('.desktop')) {
                    try {
                        const content = await fs.readFile(path.join(dir, file), 'utf-8');
                        const nameMatch = content.match(/^Name=(.*)$/m);
                        const iconMatch = content.match(/^Icon=(.*)$/m);
                        const execMatch = content.match(/^Exec=(.*)$/m);

                        if (nameMatch && execMatch) {
                            const name = nameMatch[1];
                            const execCmd = execMatch[1].split(' ')[0]; // Take command, ignore args like %u
                            const icon = iconMatch ? iconMatch[1] : '';

                            // Naive check to ignore repeats?
                            apps.push({ name, icon, exec: execCmd });
                        }
                    } catch { }
                }
            }
        } catch { }
    }

    appsCache = apps.sort((a, b) => a.name.localeCompare(b.name));
    return appsCache;
});

ipcMain.handle('system:open-with', async (_, execPath: string, filePath: string) => {
    try {
        // e.g. "code" "/path/to/file"
        // Should use spawn for detached process
        const child = spawn(execPath, [filePath], { detached: true, stdio: 'ignore' });
        child.unref();
        return true;
    } catch (e) {
        console.error('Open With failed', e);
        return false;
    }
});

ipcMain.handle('fs:get-parent', (_, dirPath: string) => {
    return path.dirname(dirPath);
});

ipcMain.handle('fs:get-home', () => {
    return os.homedir();
});

ipcMain.handle('terminal:open', async (_, initialPath: string) => {
    if (terminalProcess) {
        // Check if process is still alive?
        try {
            process.kill(terminalProcess.pid, 0);
            return; // Still running
        } catch {
            terminalProcess = null;
        }
    }

    const cwd = initialPath || os.homedir();
    terminalProcess = spawn('konsole', ['--nofork', '--workdir', cwd], {
        detached: true,
        stdio: 'ignore'
    });

    const pid = terminalProcess.pid;
    if (pid) {
        setupTerminal(pid);
    }

    // Cleanup on exit
    terminalProcess.on('exit', () => {
        terminalProcess = null;
        terminalSessionProxy = null;
    });
});

ipcMain.handle('terminal:cd', async (_, dirPath: string) => {
    if (terminalSessionProxy && dirPath) {
        try {
            const safePath = dirPath.replace(/'/g, "'\\''");
            if (terminalSessionProxy.runCommand) {
                await terminalSessionProxy.runCommand(`cd '${safePath}' && clear`);
            } else if (terminalSessionProxy.sendText) {
                await terminalSessionProxy.sendText(`cd '${safePath}'\r`);
                await terminalSessionProxy.sendText(`clear\r`);
            }
        } catch (e) {
            console.error('Failed to send cd to terminal', e);
        }
    }
});

ipcMain.handle('dialog:open-file', async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
        properties: ['openFile'],
        filters: [{ name: 'CSS Files', extensions: ['css'] }]
    });
    if (canceled) return null;
    return filePaths[0];
});

ipcMain.on('dnd:start', (event, filePath: string, iconPath?: string) => {
    // For startDrag, we need an icon. 
    // If iconPath is not provided, we should probably generate one or use a default?
    // Electron requires 'file' (absolute path) and 'icon' (path to png). 
    // Ideally we generate a dynamic icon or pass one.
    // For now we can accept an icon path or default to a generic file icon if manageable, 
    // but startDrag REQUIRES an icon.
    // Let's rely on the frontend to pass a valid icon path (maybe generated or static asset).
    // Or we can use a system icon?

    // Minimal implementation:
    event.sender.startDrag({
        file: filePath,
        icon: iconPath || filePath // Fallback to file itself? might not work for non-images
    });
});

ipcMain.handle('fs:exists', async (_, filePath: string) => {
    try {
        await fs.access(filePath);
        return true;
    } catch {
        return false;
    }
});

ipcMain.handle('system:get-drives', async () => {
    try {
        const { stdout } = await execAsync('lsblk --json -o NAME,LABEL,MOUNTPOINT,SIZE,TYPE,TRAN,RM');
        const data = JSON.parse(stdout);
        const devices = data.blockdevices || [];

        const drives: any[] = [];
        const processDevice = (dev: any) => {
            // We want devices that have a mountpoint OR children with mountpoints?
            // Actually, usually partition has mountpoint.
            if (dev.mountpoint) {
                drives.push({
                    name: dev.name,
                    label: dev.label || dev.name,
                    mountpoint: dev.mountpoint,
                    size: dev.size,
                    type: dev.type,
                    removable: dev.rm || dev.tran === 'usb',
                    usb: dev.tran === 'usb'
                });
            }
            if (dev.children) {
                dev.children.forEach(processDevice);
            }
        };

        devices.forEach(processDevice);
        return drives.filter(d => d.removable || d.mountpoint.startsWith('/run/media')); // Filter for interesting drives
    } catch (e) {
        console.error('Failed to get drives', e);
        return [];
    }
});

ipcMain.handle('system:get-storage-usage', async () => {
    try {
        // Linux: df -kP / (output in 1K blocks)
        // Output format: Filesystem 1024-blocks Used Available Capacity Mounted on
        const { stdout } = await execAsync('df -kP /');
        const lines = stdout.trim().split('\n');
        if (lines.length < 2) return null;

        const parts = lines[1].split(/\s+/);
        // parts[1] = total, parts[2] = used, parts[3] = available, parts[4] = percent
        const total = parseInt(parts[1]) * 1024; // Convert to bytes
        const used = parseInt(parts[2]) * 1024;
        const free = parseInt(parts[3]) * 1024;

        return { total, used, free };
    } catch (e) {
        console.error('Failed to get storage usage', e);
        return null;
    }
});

ipcMain.handle('fs:read-file', async (_, filePath: string) => {
    try {
        return await fs.readFile(filePath, 'utf-8');
    } catch {
        return null;
    }
});

ipcMain.handle('app:get-startup-path', async () => {
    const args = process.argv;
    // In dev, args are [electron, ., path?]
    // In prod, args are [app, path?]
    const lastArg = args[args.length - 1];

    if (!lastArg || lastArg === '.' || lastArg === process.execPath) {
        return null; // Ignore if it's the executable itself or current dir placeholder
    }

    try {
        if (path.isAbsolute(lastArg)) {
            const stats = await fs.stat(lastArg);
            if (stats.isDirectory()) {
                return lastArg;
            } else if (stats.isFile()) {
                return path.dirname(lastArg); // Open parent folder if file passed
            }
        }
    } catch (e) {
        // Path doesn't exist or other error
        return null;
    }
    return null;
});

ipcMain.handle('window:set-icon', async (_, iconType: 'light' | 'dark' | string) => {
    if (!mainWindow) return;
    // Implementation depends on having assets. 
    // --- Directory Size ---
    ipcMain.handle('system:get-directory-size', async (_, dirPath: string) => {
        // Use du -sb for bytes (Linux/GNU). -s = summary, -b = bytes
        // Note: `du -sb` is GNU. `du -sk` is POSIX (blocks). 
        // `du -b` is not available on all systems (e.g. BusyBox might vary, but usually Linux has coreutils).
        // Safer: `du -sk` and multiply by 1024.

        try {
            // Use du -sk for bytes (Linux). -s = summary, -k = 1k blocks
            const { stdout } = await execAsync(`du -sk "${dirPath}"`);
            const match = stdout.match(/^(\d+)/);
            if (match) {
                return parseInt(match[1], 10) * 1024;
            }
            return 0;
        } catch (error: any) {
            // du returns exit code 1 if it can't read some files, but still outputs total.
            // In this case, execAsync throws, but the partial stdout is in error.stdout
            if (error.stdout) {
                const match = error.stdout.match(/^(\d+)/);
                if (match) {
                    return parseInt(match[1], 10) * 1024;
                }
            }
            console.error('Failed to get directory size completely', error);
            return 0;
        }
    });

    // Search
    ipcMain.handle('system:search', async (_, directory: string, query: string, options?: { type?: 'f' | 'd', minSize?: string, maxSize?: string }) => {
        try {
            // Build find command
            const args = [directory];
            if (options?.type) args.push('-type', options.type);
            if (query) args.push('-iname', `*${query}*`);
            if (options?.minSize) args.push('-size', `+${options.minSize}`);
            if (options?.maxSize) args.push('-size', `-${options.maxSize}`);

            // Limit results logic if needed, but for now standard execution
            const { stdout } = await execFileAsync('find', args, { maxBuffer: 1024 * 1024 * 10 });

            const lines = stdout.split('\n').filter(Boolean);
            const results: any[] = []; // Use any to avoid IFile import issues

            const topLines = lines.slice(0, 100);

            for (const pathStr of topLines) {
                try {
                    const stats = await fs.stat(pathStr);
                    results.push({
                        name: path.basename(pathStr),
                        path: pathStr,
                        isDirectory: stats.isDirectory(),
                        size: stats.size,
                        mtime: stats.mtime
                    });
                } catch { }
            }
            return results;
        } catch (error) {
            console.error('Search failed:', error);
            return [];
        }
    });

    // Existing open dialog...ht have icon-light.png / icon-dark.png in resources
    // or just allowing a path.
    // user asked for "change whith theme".

    // Placeholder logic for now, or real if we had the assets.
    // For now, accept a path.
    if (path.isAbsolute(iconType)) {
        mainWindow.setIcon(iconType);
    } else {
        // Construct path to assets
        // Try to find icon in assets folder
        const possiblePaths = [
            path.join(__dirname, `../assets/icon-${iconType}.png`),
            path.join(process.resourcesPath, `assets/icon-${iconType}.png`)
        ];

        for (const p of possiblePaths) {
            try {
                // Check if exists
                await fs.access(p);
                mainWindow.setIcon(p);
                return;
            } catch { }
        }
        // console.log('Icon asset not found for:', iconType);
    }
});

// --- App Lifecycle ---

app.whenReady().then(() => {
    protocol.handle('media', (request) => {
        const filePath = request.url.slice('media://'.length);
        // Decode URI component (handle spaces etc)
        const decodedPath = decodeURIComponent(filePath);
        return net.fetch(url.pathToFileURL(decodedPath).toString());
    });
    createWindow();
});

app.on('window-all-closed', () => {
    if (terminalProcess) {
        process.kill(terminalProcess.pid);
        terminalProcess = null;
    }
    if (process.platform !== 'darwin') {
        app.quit();
    }
});

app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
        createWindow();
    }
});
