# HoshinekoFM — 代码质量审计 & 重构计划

> 生成日期: 2025-07-04
> 审查范围: 全部 `src/` (47 TS/TSX files, ~10,943 行) + `electron/` (6 TS files, ~2,221 行)

---

## 总体评分

| 维度 | 评分 (1–5) | 说明 |
|---|---|---|
| 文件长度 | 2/5 | 4 个文件超过 700 行，其中 3 个超过 1000 行 |
| 函数复杂度 | 2/5 | 多个 100+ 行函数，存在「上帝组件」 |
| 代码重复 | 2/5 | MIME→Icon 映射 160 行 × 2、列表/网格项渲染 ~200 行 × 2、冲突逻辑 ~60 行重叠 |
| 命名 / 标识符 | 3/5 | 基本可读；`AllDevice` 名不副实、部分变量用单字母、匈牙利前缀不一致 |
| 文档 / 注释 | 2/5 | 几乎零 JSDoc；导出函数 & IPC handler 无文档；较复杂的函数无算法注释 |
| 内联样式 vs CSS | 2/5 | ~50 处 `style={{}}`，约 60% 可提取为 CSS 类 |
| 架构分层 | 3/5 | 模式不错 (IPC→Service→Hook→Component)，但未合理拆分；部分边界不清晰 |
| **文件操作服务端/浏览器端边界** | **3.5/5** | 大体重操作都在 server-side，但 `importFiles` 无冲突检查，`checkConflicts` 为 N+1 IPC |

---

## 1. 文件长度问题

| 文件 | 行数 | 严重程度 | 问题 |
|------|------|----------|------|
| `src/components/FileList.tsx` | **1521** | 🔴 致命 | 渲染 (Row/GridItem/ListItem) + 橡皮框选择 + 拖拽 + 重命名 + 滚动 — 全挤在一个文件 |
| `src/App.tsx` | **1083** | 🔴 致命 | 50+ 个 state / 30+ 个回调在一个组件内；右键菜单、重命名、冲突、设备操作、创建弹窗 — 全部内联 |
| `electron/main.ts` | **1021** | 🔴 致命 | 20+ 个 IPC handler 跨 4 个领域 + 协议注册 + D-Bus + 应用生命周期 |
| `src/components/ExplorerTab.tsx` | **795** | 🟡 应当拆分 | 快捷键、排序/分组工具栏、选择逻辑、上下文菜单 — 都应抽为独立 hook |

### 拆分方案 (Phase 1–2–3)

#### `src/components/FileList.tsx` (1521 行 → ~200 行 orchestrator)
```
src/components/FileList/
├── FileList.tsx              # orchestrator (~200 行)
├── ListItem.tsx              # 列表行渲染 (~200 行)
├── GridItem.tsx              # 网格项渲染 (~200 行)
├── Header.tsx                # 分组标题 (~30 行)
├── utils.ts                  # flattenItems, computeItemBoxes, formatSize,
│                             #   getFileIconFromMime, getFileTitle, listSpacing (~300 行)
├── useSelection.ts           # 橡皮框选择 + 点击选择 + 自动滚动 (~250 行)
├── useDragDrop.ts            # 内部拖拽 + 原生文件拖拽 (~120 行)
└── useRename.ts              # 行内重命名状态 (~60 行)
```

#### `src/App.tsx` (1083 行 → ~300 行)
抽 6 个 hook：
```
src/hooks/
├── useTabs.ts                # 标签页 CRUD, activeTabId (~80 行)
├── useContextMenu.ts         # 右键菜单状态, menuItems 构建器 (~200 行)
├── useRenameDialog.ts        # 重命名弹窗 (~30 行)
├── useConflictDialog.ts      # 单/多文件冲突弹窗 (~50 行)
├── useCreateDialog.ts        # 新建文件/文件夹 (~30 行)
└── useDeviceActions.ts       # 挂载/卸载/弹出 (~80 行)
```

#### `electron/main.ts` (1021 行 → ~100 行)
```
electron/
├── main.ts                   # createWindow, media:// 协议, app 生命周期, udisks2 (~100 行)
├── handlers/
│   ├── fs.ts                 # list-dir, copy, move, trash, rename, mkdir，
│   │                         #   create-file, open, extract, exists, read-file,
│   │                         #   search, symlink ops, dir-size (~450 行)
│   ├── system.ts             # get-apps, open-with, storage-usage, drives,
│   │                         #   all-devices, mount/unmount/eject, recommended-apps (~400 行)
│   └── window.ts             # dialog:open-file, dnd:start, cache:drag-icon,
│                             #   window:set-icon, app:get-startup-path (~100 行)
```

---

## 2. 函数复杂度问题

### 过于复杂的函数 (>100 行)

| 函数 | 位置 | 行数 | 拆分建议 |
|------|------|------|----------|
| `Row` | `FileList.tsx:391–806` | ~415 | List + Grid 渲染重复内联；拆为 `<ListItem>` + `<GridItem>` 子组件 |
| `handleBackgroundMouseDown` | `FileList.tsx:1201–1405` | ~205 | 橡皮框选择 + 自动滚动 + 嵌套闭包；移入 `useSelection.ts` |
| `AppContent` | `App.tsx:54–1058` | ~1000 | 全部状态/逻辑在单组件；拆分见上 |
| `menuItems` IIFE | `App.tsx:444–623` | ~180 | 右键菜单构建内联于 JSX；移入 `useContextMenu` |
| `listDirectoryContents` | `electron/main.ts:143–347` | ~205 | 处理 6+ 种文件类型；特殊文件/挂载点处理可作为子函数或单独模块 |

---

## 3. 代码重复 — 详细对比

### 🔴 3.1 MIME → Icon 映射 (重复 ~160 行)

| | `getFileIconFromMime` (FileList.tsx:82-239) | `getIconNameForMime` (fsUtils.ts:358-501) |
|---|---|---|
| 用途 | 浏览器端渲染文件列表图标 | 服务端生成拖拽图标 |
| 注册文件类型 | 231 行 (含每种 mimetype 的 case) | 231 行 (几乎相同) |
| inode/symlink | ✅ `'link'` | ❌ 缺失 |
| inode/blockdevice | ✅ `'hard_drive'` | ❌ 缺失 |
| inode/chardevice | ✅ `'keyboard'` | ❌ 缺失 |
| inode/fifo | ✅ `'swap_vert'` | ❌ 缺失 |
| inode/socket | ✅ `'settings_ethernet'` | ❌ 缺失 |
| inode/* fallback | ✅ `'folder'` | ❌ 缺失 → fallback 到 `'insert_drive_file'` |
| 其他所有文档/音频/视频/归档 | ✅ 匹配 | ✅ 匹配 (完全同步) |

**修复方案**: 抽取 `src/utils/mimeIconMap.ts`（或 `shared/mimeIconMap.ts`），导出一份纯数据 `Record<string, string>` + 一个 `getFileIconFromMime(mime, isDirectory): string` 函数，两端同时引用。

### 🔴 3.2 列表项 / 网格项渲染 (重复 ~200 行)

`FileList.tsx` 的 `Row` 组件中：
- 列表项 (line 447–616) 和网格项 (line 618–805) 共享：
  - 图标渲染 (thumbnail vs fallback icon)
  - 挂载点 badge 叠加层
  - 损坏 symlink 图标
  - 事件处理器 (click, double-click, context menu, dragStart, dragOver, dragLeave, drop)
  - 重命名输入框

**修复方案**: 抽取 `<FileItem>` 通用组件，通过 `variant` prop 控制布局；或拆为 `<ListItem>` + `<GridItem>` 但共享 `<FileItemCore>` 内部渲染。

### 🟡 3.3 冲突解决逻辑 (重复 ~60 行)

| | `handleDropOnTarget` (ExplorerTab.tsx:262–328) | `pasteFiles` (fileOperations.ts:238–315) |
|---|---|---|
| 流程 | checkConflicts → dialog → renameMap → 循环 copy/move → toast | checkConflicts → dialog → renameMap → 循环 copy/move → toast |
| 差异 | 额外处理 `usedNames` Set | 额外处理 `conflictNames` Set |

**修复方案**: 抽取 `resolveConflictsAndBatchCopyOrMove()` 到 `fileOperations.ts`，接收一个进度回调。

### 🟡 3.4 分区/磁盘渲染 (重复 ~120 行)

`Sidebar.tsx` line 162–238 (带 children 的 disk) 和 line 241–298 (独立 disk) 的 JSX 几乎相同。

**修复方案**: 抽取 `<SidebarPartitionItem>` 组件。

---

## 4. 文件操作服务端/浏览器端边界审计 🔑

> *这是核心安全/正确性审查项：判断文件类型、移动/复制/粘贴等操作。*

### ✅ 正确设计的操作

| 操作 | 服务端实现 | 边界 |
|------|-----------|------|
| 列目录 | `fs.readdir` + `fs.stat` + `/proc/mounts` + `/sys/class/block/*` | 全部服务端 |
| 复制 | `fs.cp(source, dest, { recursive: true, force: false })` | 全部服务端 |
| 移动 | `fs.rename(source, dest)` | 全部服务端 |
| 删除 | `shell.trashItem(filePath)` | 全部服务端 |
| 重命名 | `fs.rename(oldPath, newPath)` | 全部服务端 |
| 新建文件 | `fs.writeFile(filePath, '', 'utf-8')` | 全部服务端 |
| 新建文件夹 | `fs.mkdir(dirPath, { recursive: true })` | 全部服务端 |
| 解压 | `execAsync('unzip ...')` / `execAsync('tar -xf ...')` | 全部服务端 |
| MIME 检测 | 4-tier: magic bytes → 扩展名 → 基本名 → `file` 命令 | 全部服务端 |
| 挂载/卸载 | `udisksctl mount/unmount/power-off` | 全部服务端 |
| 外部→App 拖放 | `fs.cp` 逐文件 | 全部服务端 |
| App→外部 拖放 | Electron 原生 `startDrag` API | 全部服务端 |

### 🔴 严重问题

#### 4a. `importFiles()` 无冲突检查且静默吞错

`fileOperations.ts:330–350` — 拖放外部文件到 App 时调用，直接逐个复制，不做任何冲突检查：

```ts
for (const entry of fileEntries) {
  try {
    await window.electron.copyFile(entry.path, destPath);  // 直接复制，不检查
    count++;
  } catch (e) {
    console.error(`导入 ${name} 失败:`, e);  // 仅 console，无 toast 提示
  }
}
```

**后果**：
- 若同名文件存在，`fs.cp` 的 `force: false` 会抛 `EEXIST`，用户**无感知**获知文件被静默跳过
- `count` 不递增大错误 — 但 toast 只报 `count` 数，用户不知道有多少文件失败了
- 对比 `pasteFiles()`：后者有完整的冲突检测 + 冲突对话框 + 重命名/跳过选项

**修复**：参考 `pasteFiles()` 添加冲突预检查 + 用户确认流程；至少应显示失败 toast。

#### 4b. `checkConflicts()` — N+1 IPC 往返

`fileConflict.ts:44–53` — 逐文件调用 `window.electron.exists(destPath)`：

```ts
for (const entry of entries) {
  const exists = await window.electron.exists(destPath);  // 每次一个 IPC 往返
  if (exists) conflicts.push(...);
}
```

100 个文件 = 100 次 IPC `invoke` 往返。**同时存在 TOCTOU 竞态条件**：`exists()` 返回 `false` 后、`copyFile()` 执行前，另一个进程可能创建同名文件。

**修复**：在 `main.ts` 添加 `fs:exists-batch` handler，接受 `string[]` 返回 `Record<string, boolean>`，一次 IPC 完成。

#### 4c. `trashFiles()` — N 次独立 IPC，无原子性

`fileOperations.ts:157–163` — 循环调用 `window.electron.trashFile(p)`：

```ts
for (const p of paths) {
  await window.electron.trashFile(p);  // N 次单独 IPC
}
```

若中途失败，部分文件已删除、部分未删除，无回滚可能。

#### 4d. Symlink 指向块设备时 `detectMime()` 可能挂起

`electron/main.ts:226–236` — 对 symlink 调用 `fs.stat()` 以解析目标，然后对目标调用 `detectMime()`。若 symlink 指向 `/dev/sda`（块设备），`detectMime()` 会尝试读取前 16 bytes — 从块设备读取可能永久挂起。

**修复**：先检查 `resolvedStats.isBlockDevice()` / `isCharacterDevice()` / `isFIFO()` / `isSocket()`，再决定是否调用 `detectMime()`。

### 🟡 轻微问题

| 问题 | 位置 | 说明 |
|------|------|------|
| `FileSystemService.copy/move/trash/rename/mkdir` — 死代码 | `src/services/FileSystemService.ts:8–27` | 这几个函数和 `fileOperations.ts` 功能重叠，且代码中无任何调用方 |
| `preload.ts` 暴露的接口无输入校验 | `electron/preload.ts` | 虽然有 `contextIsolation`，但是更佳做法是在 IPC handler 中 normalize path  |
| `renameFile` 在浏览器端做了单独的 `exists()` 检查 | `fileOperations.ts:117` | 先 `exists` 再 `rename` 存在 TOCTOU；应在服务端一体检查 |

---

## 5. 命名 & 标识符问题

| 当前命名 | 问题 | 建议 |
|----------|------|------|
| `AllDevice` | 名字暗示是集合，实际是单个设备接口 | `DeviceInfo` |
| `AppContent` | 泛型名；它是主壳/布局 | `AppShell` 或 `AppLayout` |
| `IFile` / `AllDevice` | 匈牙利前缀不一致 (`I` vs 无前缀) | 统一使用 `FileEntry` / `DeviceInfo` |
| `d` / `c` (循环变量) | 在复杂 JSX 中用单字母 | 改为 `device` / `conflictState` |
| `setSingleConflict(singleConflict)` | setter 和 getter 同名 | `setSingleConflictState` |
| `_draggedPaths` / `_pendingNativeDragPaths` | 模块级可变变量，下划线前缀 | 移入 `DragContext` 或改为 React ref |
| `mountpoint` vs `mountPoint` vs `mount_point` | 多处不一致 | 统一为 `mountpoint`（与 Linux 一致） |

---

## 6. 文档与注释

### 现状：几乎零 JSDoc

| 文件 | 缺少 |
|------|------|
| `App.tsx` | 所有 handler 无 JSDoc；50+ 个 state 声明无任何说明 |
| `FileList.tsx` | `Row`, `flattenItems`, `computeItemBoxes`, `handleBackgroundMouseDown` 无 JSDoc；橡皮框选择算法 (~200 行) 无算法级注释 |
| `ExplorerTab.tsx` | 仅 line 592 一行中文注释；其余零文档 |
| `electron/main.ts` | `listDirectoryContents` (最复杂函数), IPC handler, `getMountMap` 无 JSDoc |
| `electron/fsWatcher.ts` | 无 watch/debounce 行为文档 |

**仅有文档的文件**：`electron/fsUtils.ts` (有 JSDoc)、`electron/mimeMap.ts` (清晰注释)、`electron/preload.ts` (段落注释)。

### 需添加

- 所有 `export` 函数/类型 → JSDoc（特别是 IPC handler 和 React hook）
- 橡皮框选择算法 → 顶层算法说明注释
- `listDirectoryContents` → `@returns` 说明各字段何时存在
- 每个 IPC handler → 说明接受的参数、可能返回的错误

---

## 7. 内联样式 vs CSS

### 问题：~50 处 `style={{}}`，其中 ~30 处应提取为 CSS 类

| 文件 | 内联样式块数 | 严重样本 |
|------|------------|----------|
| `App.tsx` | ~15 | 根 div、终端面板 header、内容区、占位符状态 |
| `FileList.tsx` | ~25 | 列表项容器、网格项容器、行包裹层、选择框 |
| `ExplorerTab.tsx` | ~10 | 顶栏布局、搜索结果栏、拖放目标 div |

合法内联样式 (保留)：
- 动态尺寸：`width: ${data.iconSize}px`
- 鼠标坐标定位：选择框 `top: selectionBox.y`
- 计算值：`gap: ${sp.gap}px`

需提取为 CSS 类：
- `.app-shell` — 根 flex 容器
- `.explorer-tab` — tab 主容器
- `.explorer-tab--hidden` — `display: none`
- `.explorer-topbar` — 面包屑/导航栏
- `.file-item-container` — 文件项弹性容器 (同时用于 list 和 grid)
- `.terminal-panel` — 底部面板
- `.search-info-bar` — 搜索结果信息条

---

## 8. 重构计划 (优先级排序)

### Phase 1: 抽取 App.tsx 的 Hooks (低风险, 4–6h)

```
src/hooks/
├── useTabs.ts               # tabs, activeTabId, handleAddTab, handleCloseTab
├── useContextMenu.ts        # contextMenu, bgMenuItems, deviceContextMenu
├── useRenameDialog.ts       # renameDialogOpen, renameFile, newName, handleRename
├── useConflictDialog.ts     # singleConflict, multiConflict, handleConflictDialog
├── useCreateDialog.ts       # createDialog, handleCreateDialog
└── useDeviceActions.ts      # handleDeviceMount/Unmount/Eject
```

### Phase 2: 拆分 electron/main.ts (中风险, 3–4h)

```
electron/
├── main.ts                  # createWindow, media:// 协议, 生命周期 (~100 行)
├── handlers/fs.ts           # fs:* handlers (~450 行)
├── handlers/system.ts       # system:* handlers (~400 行)
└── handlers/window.ts       # dialog, dnd, icon, startup (~100 行)
```

### Phase 3: 拆分 FileList.tsx (中风险, 6–8h)

5 个子组件 + 3 个 hook + 1 个 util 文件（方案见 §1）

### Phase 4: 修复文件操作边界 (🔑 高优先级, 3–4h)

1. **`importFiles()` 加冲突检查 + 错误提示** (severity: 🔴)
   - 调用 `checkConflicts()` 或改用 `pasteFiles()` 的重构版本
   - 添加失败 toast

2. **`checkConflicts()` → 批量 IPC** (severity: 🔴)
   - 添加 `fs:exists-batch` handler 在 `main.ts`
   - `checkConflicts()` 改为一次 IPC 调用

3. **`trashFiles()` → 批量 IPC** (severity: 🟡)
   - 添加 `fs:trash-batch` handler 在 `main.ts`
   - 或改为逐个删除但报清楚总数

4. **Symlink → Block Device 修复** (severity: 🟢 边缘场景)
   - `listDirectoryContents` 在调用 `detectMime()` 前检查 stat 类型

### Phase 5: 消除代码重复 (低风险, 3–4h)

1. **统一 MIME→Icon 映射** — 抽取 `src/utils/mimeIconMap.ts`
2. **抽取共享冲突解决** — `resolveConflictsAndBatchOp()` 到 `fileOperations.ts`
3. **抽取 `<SidebarPartitionItem>`** 子组件
4. **移除 `FileSystemService` 死代码** — `copy/move/trash/rename/mkdir`

### Phase 6: 样式迁移 (低风险, 3–5h)

将 ~30 处内联风格迁移到 CSS 类

### Phase 7: 补充文档 (低风险, 2–3h)

JSDoc：导出函数、IPC handler、关键算法

---

## 9. 指标对比

| 指标 | 当前 | 目标 |
|------|------|------|
| 最大文件行数 | 1521 (`FileList.tsx`) | <400 |
| >500 行的文件数 | 7 | 0 |
| >200 行的函数数 | 3 | 0 |
| 重复 MIME→Icon 代码 | 160 行 × 2 | 0 (共享模块) |
| 重复项渲染代码 | ~200 行 × 2 | 0 (共享组件) |
| 内联 style 块 | ~50 | <20 (仅动态值) |
| `importFiles` 冲突检查 | ❌ 无 | ✅ 有 |
| `checkConflicts` IPC 调用 | N 次 | 1 次 |
| JSDoc 覆盖率 | <5% 导出函数 | >80% |
| 右键菜单构建位置 | 3 处 | 1 处 (`useContextMenu`) |

---

## 10. 预估工时

| Phase | 任务 | 预估时间 | 风险 |
|-------|------|----------|------|
| Phase 1 | 抽取 App.tsx hooks | 4–6h | 低 |
| Phase 2 | 拆分 electron/main.ts | 3–4h | 中 |
| Phase 3 | 拆分 FileList.tsx | 6–8h | 中 |
| Phase 4 | 修复文件操作边界 | 3–4h | 中 |
| Phase 5 | 消除代码重复 | 3–4h | 低 |
| Phase 6 | 样式迁移到 CSS | 3–5h | 低 |
| Phase 7 | 补充 JSDoc | 2–3h | 低 |
| **合计** | | **24–34h** | |

---

*Phase 4 (文件操作边界修复) 涉及核心功能正确性，建议最优先执行。Phase 1–3 (大文件拆分) 可从任意顺序开始，互不相依赖。*
