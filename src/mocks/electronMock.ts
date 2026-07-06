/**
 * Electron API Mock —— 在 Tauri 迁移阶段替代 window.electron。
 *
 * 当 Tauri 后端尚未实现所有功能时，此 mock 允许前端在浏览器中正常打开和调试。
 * 页面初次加载时通过 main.tsx 注入：
 *   `window.electron = electronMock;`
 */

import type { AllDevice, IFile } from "../types/files";
import type { StartJobParams, JobProgress, JobComplete } from "../types/electron";

const noopDisposer = (): void => {};

// ─── Mock 文件系统 ──────────────────────────────────────────────────────

/** 基础时间戳，避免 mtime 每次重新计算 */
const MOCK_MTIME = new Date("2026-07-01T12:00:00Z");

/** 快速构造一个 IFile 条目 */
function file(path: string, opts: Partial<IFile> = {}): IFile {
  const name = path.split("/").pop()!;
  return {
    name,
    path,
    isDirectory: false,
    size: 0,
    mtime: MOCK_MTIME,
    mime: null,
    ...opts,
  };
}

/** 快速构造一个目录条目 */
function folder(path: string, opts: Partial<IFile> = {}): IFile {
  return file(path, { isDirectory: true, mime: "inode/directory", ...opts });
}

/** Mock 文件树（path → entries） */
const MOCK_FS: Record<string, IFile[]> = {
  "/tmp/mock-home": [
    folder("/tmp/mock-home/Documents"),
    folder("/tmp/mock-home/Downloads"),
    folder("/tmp/mock-home/Projects"),
    folder("/tmp/mock-home/Pictures"),
    file("/tmp/mock-home/hello.txt", { mime: "text/plain", size: 42 }),
    file("/tmp/mock-home/notes.md", { mime: "text/markdown", size: 2048 }),
    file("/tmp/mock-home/portrait.png", { mime: "image/png", size: 102400 }),
    file("/tmp/mock-home/archive.zip", { mime: "application/zip", size: 524288 }),
    file("/tmp/mock-home/script.sh", { mime: "application/x-shellscript", size: 512 }),
    file("/tmp/mock-home/config.json", { mime: "application/json", size: 256 }),
  ],
  "/tmp/mock-home/Documents": [
    file("/tmp/mock-home/Documents/resume.pdf", { mime: "application/pdf", size: 128000 }),
    file("/tmp/mock-home/Documents/report.docx", { mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", size: 256000 }),
    file("/tmp/mock-home/Documents/budget.xlsx", { mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", size: 64000 }),
    folder("/tmp/mock-home/Documents/Archive"),
  ],
  "/tmp/mock-home/Documents/Archive": [
    file("/tmp/mock-home/Documents/Archive/old-tax-2025.pdf", { mime: "application/pdf", size: 256000 }),
    file("/tmp/mock-home/Documents/Archive/scan.png", { mime: "image/png", size: 409600 }),
  ],
  "/tmp/mock-home/Downloads": [
    file("/tmp/mock-home/Downloads/ubuntu-24.04.iso", { mime: "application/x-iso9660-image", size: 5242880000 }),
    file("/tmp/mock-home/Downloads/firefox.tar.bz2", { mime: "application/x-bzip2", size: 89000000 }),
  ],
  "/tmp/mock-home/Projects": [
    folder("/tmp/mock-home/Projects/hnfm"),
    folder("/tmp/mock-home/Projects/website"),
    file("/tmp/mock-home/Projects/README.md", { mime: "text/markdown", size: 1024 }),
  ],
  "/tmp/mock-home/Projects/hnfm": [
    folder("/tmp/mock-home/Projects/hnfm/src"),
    folder("/tmp/mock-home/Projects/hnfm/src-tauri"),
    file("/tmp/mock-home/Projects/hnfm/package.json", { mime: "application/json", size: 2048 }),
    file("/tmp/mock-home/Projects/hnfm/tsconfig.json", { mime: "application/json", size: 512 }),
  ],
  "/tmp/mock-home/Projects/website": [
    file("/tmp/mock-home/Projects/website/index.html", { mime: "text/html", size: 4096 }),
    file("/tmp/mock-home/Projects/website/style.css", { mime: "text/css", size: 2048 }),
  ],
  "/tmp/mock-home/Projects/hnfm/src": [
    file("/tmp/mock-home/Projects/hnfm/src/App.tsx", { mime: "text/typescript", size: 16384 }),
    file("/tmp/mock-home/Projects/hnfm/src/main.tsx", { mime: "text/typescript", size: 512 }),
    folder("/tmp/mock-home/Projects/hnfm/src/components"),
  ],
  "/tmp/mock-home/Projects/hnfm/src-tauri": [
    file("/tmp/mock-home/Projects/hnfm/src-tauri/Cargo.toml", { mime: "text/plain", size: 1024 }),
    folder("/tmp/mock-home/Projects/hnfm/src-tauri/src"),
  ],
  "/tmp/mock-home/Pictures": [
    file("/tmp/mock-home/Pictures/vacation.jpg", { mime: "image/jpeg", size: 2048000 }),
    file("/tmp/mock-home/Pictures/screenshot.png", { mime: "image/png", size: 512000 }),
    file("/tmp/mock-home/Pictures/wallpaper.webp", { mime: "image/webp", size: 1024000 }),
    folder("/tmp/mock-home/Pictures/2025"),
  ],
  "/tmp/mock-home/Pictures/2025": [
    file("/tmp/mock-home/Pictures/2025/spring.jpg", { mime: "image/jpeg", size: 1024000 }),
    file("/tmp/mock-home/Pictures/2025/summer.jpg", { mime: "image/jpeg", size: 1536000 }),
  ],
  "/": [
    folder("/tmp"),
    folder("/etc"),
    folder("/usr"),
    folder("/home"),
    file("/.bashrc", { mime: "text/plain", size: 4096 }),
  ],
};

/** Mock 任务回调存储（模拟 main process 的 job 系统） */
const jobCallbacks = new Map<string, {
  progressCbs: Set<(data: JobProgress) => void>;
  completeCbs: Set<(data: JobComplete) => void>;
}>();
let jobIdCounter = 0;

const electronMock = {
  // ─── Theme ──────────────────────────────────────────────────────────────
  getThemeCss: async () => null,
  setIcon: async (_iconType: string) => {},

  // ─── File System ────────────────────────────────────────────────────────
  async listDir(_path: string) {
    const entries = MOCK_FS[_path] ?? [];
    console.log(`[mock] listDir("${_path}") → ${entries.length} entries`);
    return { data: entries, actualPath: _path };
  },
  async getParentPath(path: string) {
    const idx = path.lastIndexOf("/");
    return idx <= 0 ? "/" : path.substring(0, idx);
  },
  async getHomePath() {
    return "/tmp/mock-home";
  },
  async getHomeMap() {
    return {};
  },
  async getPlaces() {
    return [
      { name: "Home", path: "/tmp/mock-home", icon: "home" },
      { name: "Root", path: "/", icon: "folder" },
      { name: "Documents", path: "/tmp/mock-home/Documents", icon: "description" },
      { name: "Downloads", path: "/tmp/mock-home/Downloads", icon: "download" },
      { name: "Pictures", path: "/tmp/mock-home/Pictures", icon: "image" },
    ];
  },
  async copyFile(_source: string, _dest: string) {
    console.log(`[mock] copy ${_source} → ${_dest}`);
    return true;
  },
  async moveFile(_source: string, _dest: string) {
    console.log(`[mock] move ${_source} → ${_dest}`);
    return true;
  },
  async trashFile(_path: string) {
    console.log(`[mock] trash ${_path}`);
    return true;
  },
  async renameFile(_oldPath: string, _newPath: string) {
    console.log(`[mock] rename ${_oldPath} → ${_newPath}`);
    return true;
  },
  async createDirectory(_path: string) {
    console.log(`[mock] mkdir ${_path}`);
    return true;
  },
  async createFile(_path: string) {
    console.log(`[mock] create file ${_path}`);
    return true;
  },
  async openPath(_path: string) {
    console.log(`[mock] open ${_path}`);
    return "ok";
  },
  async extractFile(_path: string) {
    console.log(`[mock] extract ${_path}`);
    return true;
  },
  async exists(_path: string) {
    return _path in MOCK_FS;
  },
  async existsBatch(paths: string[]) {
    return Object.fromEntries(paths.map((p) => [p, p in MOCK_FS]));
  },
  async getSymlinkTarget(_path: string) {
    return { isSymlink: false, targetExists: false };
  },
  async checkSymlinks(paths: string[]) {
    return paths.map((path) => ({ path, isSymlink: false }));
  },
  async realpath(path: string) {
    return path;
  },
  async getDirectorySize(_path: string) {
    // 递归计算 mock 文件系统中该目录下的所有文件大小
    const visited = new Set<string>();
    const walk = (p: string): number => {
      if (visited.has(p)) return 0;
      visited.add(p);
      const entries = MOCK_FS[p];
      if (!entries) return 0;
      let size = 0;
      for (const entry of entries) {
        if (entry.isDirectory) {
          size += walk(entry.path);
        } else {
          size += entry.size;
        }
      }
      return size;
    };
    return walk(_path);
  },

  // ─── Applications ───────────────────────────────────────────────────────
  async getApps() {
    return [];
  },
  async getRecommendedApps(_path: string) {
    return [];
  },
  async openWith(_exec: string, _path: string, _desktopFile?: string): Promise<true | string> {
    console.log(`[mock] openWith ${_exec} ${_path}`);
    return true;
  },
  async openFileDialog() {
    return null;
  },
  async readFile(_path: string) {
    return null;
  },
  async getStartupPath() {
    return "/tmp/mock-home";
  },

  // ─── Search ─────────────────────────────────────────────────────────────
  async search(_directory: string, _query: string, _options?: Record<string, unknown>) {
    return [];
  },

  // ─── Storage ────────────────────────────────────────────────────────────
  async getStorageUsage() {
    return null;
  },

  // ─── Drives & Devices ───────────────────────────────────────────────────
  async getDrives() {
    return [];
  },
  async getAllDevices(): Promise<AllDevice[]> {
    return [];
  },
  async getMountMap() {
    return {};
  },
  async mountDevice(_devicePath: string) {
    return { success: false, error: "[mock] mount not available" };
  },
  async unmountDevice(_devicePath: string) {
    return { success: false, error: "[mock] unmount not available" };
  },
  async ejectDevice(_devicePath: string) {
    return { success: false, error: "[mock] eject not available" };
  },
  onDeviceChange(_callback: (devices: AllDevice[]) => void) {
    return noopDisposer;
  },
  async hasDeviceWatcher() {
    return false;
  },

  // ─── PTY (unsupported in mock) ──────────────────────────────────────────
  async ptySpawn(_cwd: string) {
    console.warn("[mock] ptySpawn not available");
    return -1;
  },
  ptyWrite(_pid: number, _data: string) {
    // no-op
  },
  ptyResize(_pid: number, _cols: number, _rows: number) {
    // no-op
  },
  ptyKill(_pid: number) {
    // no-op
  },
  ptyOnData(_pid: number, _callback: (data: string) => void) {
    return noopDisposer;
  },
  ptyOnExit(_pid: number, _callback: () => void) {
    return noopDisposer;
  },

  // ─── File Watching ──────────────────────────────────────────────────────
  async watchDirectory(_dir: string) {
    // no-op
  },
  async unwatchDirectory(_dir: string) {
    // no-op
  },
  onDirChanged(_callback: (dir: string) => void) {
    return noopDisposer;
  },

  // ─── Drag (Tauri 使用不同的 API，Electron 版本不再使用) ──────────────────
  startDrag(_paths: string | string[]) {
    const paths = Array.isArray(_paths) ? _paths : [_paths];
    console.log("[mock] startDrag → 模拟拖出文件到外部:", paths);
  },
  cacheDragIcon(_name: string, _pngBase64: string) {
    // no-op
  },

  // ─── Job System ─────────────────────────────────────────────────────────
  async startJob(params: StartJobParams) {
    const jobId = `mock-job-${++jobIdCounter}`;
    const total = params.items.length;

    // 存储回调容器
    jobCallbacks.set(jobId, {
      progressCbs: new Set(),
      completeCbs: new Set(),
    });

    // 模拟异步任务完成（短暂延迟后触发进度 + 完成回调）
    setTimeout(() => {
      const cbs = jobCallbacks.get(jobId);
      if (!cbs) return;

      // 模拟 100% 进度
      for (const cb of cbs.progressCbs) {
        cb({ jobId, current: total, total, errors: [] });
      }

      // 模拟全部成功完成
      for (const cb of cbs.completeCbs) {
        cb({ jobId, success: total, fail: 0, errors: [], cancelled: false });
      }

      jobCallbacks.delete(jobId);
    }, 100);

    return jobId;
  },
  async cancelJob(jobId: string) {
    const cbs = jobCallbacks.get(jobId);
    if (cbs) {
      for (const cb of cbs.completeCbs) {
        cb({ jobId, success: 0, fail: 0, errors: [], cancelled: true });
      }
      jobCallbacks.delete(jobId);
    }
  },
  onJobProgress(jobId: string, callback: (data: JobProgress) => void) {
    const cbs = jobCallbacks.get(jobId);
    if (cbs) cbs.progressCbs.add(callback);
    return () => {
      const cbs = jobCallbacks.get(jobId);
      if (cbs) cbs.progressCbs.delete(callback);
    };
  },
  onJobComplete(jobId: string, callback: (data: JobComplete) => void) {
    const cbs = jobCallbacks.get(jobId);
    if (cbs) cbs.completeCbs.add(callback);
    return () => {
      const cbs = jobCallbacks.get(jobId);
      if (cbs) cbs.completeCbs.delete(callback);
    };
  },
};

export default electronMock;
