/**
 * 后端 → 前端推送的事件载荷类型定义。
 *
 * 前端通过 `listen("hf:xxx", callback)` 接收这些事件。
 * 所有 navigation/action 操作后端通过 event push，前端只发意图。
 */

/** `hf:tabs` 事件载荷 */
export interface TabsPayload {
  tabs: TabInfo[];
  active_tab_id: number;
}

/** Tab 信息（轻量，不含文件内容） */
export interface TabInfo {
  id: number;
  title: string;
  nav_target: NavTarget;
}

/** 导航目标 */
export interface NavTarget {
  Dashboard?: null;
  Filesystem?: string;
}

/** `hf:nav-state` 事件载荷 */
export interface NavStatePayload {
  tab_id: number;
  target: NavTarget;
  can_go_back: boolean;
  can_go_forward: boolean;
}

/** `hf:file-list` 事件载荷 — WatchDelta */
export type WatchDelta =
  | { Reset: FileEntry[] }
  | { Upsert: FileEntry }
  | { UpsertBatch: FileEntry[] }
  | { Remove: string }
  | { Rename: { from: string; to: string } }
  | { Inaccessible: { path: string; ancestor: string; level: number; reason: string } }
  | { Recovering: { path: string; ancestor: string; level: number } }
  | { FatalError: { path: string; reason: string } }
  | { ConnectionLost: { watch_id: number; reason: string; reconnecting: boolean } };

/** 文件条目（后端协议 File 类型） */
export interface FileEntry {
  name: string;
  path: string;
  size: number;
  modified: number; // SystemTime as unix timestamp
  is_directory: boolean;
  is_symlink: boolean;
  permissions: number;
  owner_uid: number;
  owner_gid: number;
  mime: string | null;
  thumbnail: number[] | null; // Vec<u8> as byte array
}

/** `hf:breadcrumbs` 事件 — 单个路径段信息 */
export interface BreadcrumbEntry {
  name: string;
  path: string;
  is_symlink: boolean;
  symlink_target: string | null;
  is_mount_point: boolean;
  mount_source: string | null;
  is_home: boolean;
  home_username: string | null;
  accessible: boolean;
}

/** `hf:breadcrumbs` 事件载荷 */
export interface BreadcrumbsPayload {
  tab_id: number;
  entries: BreadcrumbEntry[];
}
export interface DashboardData {
  storage: {
    total_bytes: number;
    used_bytes: number;
    free_bytes: number;
  };
  common_locations: {
    name: string;
    path: string;
    exists: boolean;
  }[];
}

/** `hf:selection` 事件载荷 */
export type SelectionPayload = string[];

/** `hf:clipboard` 事件载荷 */
export interface ClipboardState {
  operation: "Copy" | "Cut" | null;
  files: string[];
}

/** 从 NavTarget 提取文件路径 */
export function navTargetPath(target: NavTarget): string {
  return target.Filesystem ?? "app://dashboard";
}

/** 从 NavTarget 判断是否为 Dashboard */
export function isDashboard(target: NavTarget): boolean {
  return target.Dashboard !== undefined;
}
