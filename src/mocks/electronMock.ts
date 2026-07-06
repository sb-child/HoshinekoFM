/**
 * Electron API Mock —— 在 Tauri 迁移阶段替代 window.electron。
 *
 * 当 Tauri 后端尚未实现所有功能时，此 mock 允许前端在浏览器中正常打开和调试。
 * 页面初次加载时通过 main.tsx 注入：
 *   `window.electron = electronMock;`
 */

import type { AllDevice } from "../types/files";
import type { StartJobParams, JobProgress, JobComplete } from "../types/electron";

const noopDisposer = (): void => {};

const electronMock = {
  // ─── Theme ──────────────────────────────────────────────────────────────
  getThemeCss: async () => null,
  setIcon: async (_iconType: string) => {},

  // ─── File System ────────────────────────────────────────────────────────
  async listDir(_path: string) {
    return { data: [], actualPath: _path };
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
    return false;
  },
  async existsBatch(paths: string[]) {
    return Object.fromEntries(paths.map((p) => [p, false]));
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
    return 0;
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
    return null;
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
    console.warn("[mock] startDrag not available");
  },
  cacheDragIcon(_name: string, _pngBase64: string) {
    // no-op
  },

  // ─── Job System ─────────────────────────────────────────────────────────
  async startJob(_params: StartJobParams) {
    console.warn("[mock] startJob not available");
    return "mock-job-id";
  },
  async cancelJob(_jobId: string) {
    // no-op
  },
  onJobProgress(_jobId: string, _callback: (data: JobProgress) => void) {
    return noopDisposer;
  },
  onJobComplete(_jobId: string, _callback: (data: JobComplete) => void) {
    return noopDisposer;
  },
};

export default electronMock;
