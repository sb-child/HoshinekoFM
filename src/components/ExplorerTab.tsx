import React, { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from '@tauri-apps/api/core';
import { showToast, showProgressToast, updateProgress, finishToast } from '../utils/toast';
import { useClipboard } from '../contexts/ClipboardContext';
import { StatusBar } from './StatusBar';
import { FileList } from './FileList';
import { IconButton } from './IconButton';
import { Icon } from './Icon';
import type { IFile } from '../types/files';
import type { WatchDelta, FileEntry, BreadcrumbEntry } from '../types/tauriEvents';
import {
  renameFile,
  trashFiles,
  pasteFiles,
  createDirectory,
  createFile,
  openFile,
  copyToClipboard,
  cutToClipboard,
} from '../utils/fileOperations';
import { FileDndProvider, type DndDragData, parseDropPaths, useSetDragOver } from '../utils/dnd';
import type { DragEndEvent } from '@dnd-kit/core';

import { Omnibar } from './Omnibar';
import { Dashboard } from './Dashboard';
import { t } from '../i18n';
import { getSemanticGroup, GROUP_ORDER } from '../utils/fileUtils';
import type { ContextMenuItem } from './ContextMenu';
import {
  checkConflicts,
  generateSafeName,
  splitNameExt,
  prepareDestParent,
  type ConflictEntry,
  type ConflictResult,
} from '../utils/fileConflict';

/** 将后端 FileEntry 映射为前端 IFile */
function mapBackendFile(f: FileEntry): IFile {
  return {
    name: f.name,
    path: f.path,
    isDirectory: f.is_directory,
    size: f.size,
    mtime: new Date(f.modified * 1000),
    mime: f.mime,
  };
}

interface ExplorerTabProps {
    tabId: number;
    isActive: boolean;
    initialPath: string;
    onPathChange: (path: string) => void;
    onContextMenu: (e: React.MouseEvent, file: IFile | null) => void;
    onBgMenuItems: (items: ContextMenuItem[]) => void;
    onOpenWithFile: (file: IFile) => void;
    onPropertiesFile: (file: IFile) => void;
    onOpenTerminalAt: (path: string) => void;
    onCreateDialog: (type: 'file' | 'folder', defaultName: string, existingNames: string[]) => Promise<string | null>;
    onConflictDialog: (conflicts: ConflictEntry[], destDir: string, existingNames: string[], sourcePath?: string, operation?: "move" | "copy") => Promise<ConflictResult>;
    showHiddenFiles: boolean;
    iconSize: number;
    viewMode: 'grid' | 'list';
    filledIcons: boolean;
    onMountDevice?: (devicePath: string) => Promise<{ success: boolean; mountpoint?: string; error?: string }>;
    marqueeEnabled: boolean;
    /** 刷新当前 tab 文件列表（F5） */
    onRefresh?: () => void;
    /** 面包屑条目（来自后端 hf:breadcrumbs 事件） */
    breadcrumbs: BreadcrumbEntry[];
}

export function ExplorerTab({ tabId, isActive, initialPath, onPathChange, onContextMenu, onBgMenuItems, onOpenWithFile, onPropertiesFile, onOpenTerminalAt, onCreateDialog, onConflictDialog, showHiddenFiles, iconSize, viewMode, filledIcons, onMountDevice, marqueeEnabled, onRefresh, breadcrumbs }: ExplorerTabProps) {
  const [currentPath, setCurrentPath] = useState(initialPath);
  const [files, setFiles] = useState<IFile[]>([]);
  const [hoveredFile, setHoveredFile] = useState<IFile | null>(null);
  /** 目录不可访问状态（由后端 Inaccessible 事件设置） */
  const [inaccessible, setInaccessible] = useState<{ path: string; ancestor: string; reason: string } | null>(null);
  const lastNavRef = useRef<{ path: string; time: number } | null>(null);
  const unlistenFileListRef = useRef<UnlistenFn | null>(null);
  const { copy, cut, clipboard, clear: clearClipboard } = useClipboard();

  // Search State
  const currentPathRef = useRef(currentPath);
  // eslint-disable-next-line react-hooks/refs -- keep ref in sync for stable callbacks during render
  currentPathRef.current = currentPath;
  const initialPathRef = useRef(initialPath);
  initialPathRef.current = initialPath;

  /** 搜索（待对接 fff-search crate） */
  const handleSearch = async (_query: string) => {
    showToast(t('toast.searching') || '搜索功能开发中', 'info');
  };

  /**
   * 浏览器环境下的外部拖入处理。
   *
   * Tauri 环境下由 App.tsx 的 onDragDropEvent 处理。
   * 浏览器环境下需要通过 HTML5 DnD 的 drop 事件处理。
   * 这不会与 dnd-kit 冲突，因为 dnd-kit 使用 Pointer Events。
   */
  const setDragOverPath = useSetDragOver();
  const setDragOverPathRef = useRef(setDragOverPath);
  setDragOverPathRef.current = setDragOverPath;

  useEffect(() => {
    // HTML5 DnD handler 在 Tauri 和浏览器环境下都生效
    // Tauri 的 onDragDropEvent 处理 text/uri-list（Nautilus 等）
    // 这里的 handler 处理 text/plain（VSCode 等）
    // 两者共存，互不冲突

    const handleDocDragOver = (e: DragEvent) => {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";

      // 检测鼠标下方的文件夹元素，更新 DragOverContext
      const el = document.elementFromPoint(e.clientX, e.clientY);
      const folderItem = el?.closest('[data-droppable-id^="folder:"]');
      if (folderItem) {
        const droppableId = folderItem.getAttribute("data-droppable-id");
        const folderPath = droppableId?.replace("folder:", "");
        if (folderPath) {
          setDragOverPathRef.current(folderPath);
          return;
        }
      }
      setDragOverPathRef.current(null);
    };

    const handleDocDrop = (e: DragEvent) => {
      setDragOverPathRef.current(null);
      const dt = e.dataTransfer;
      if (!dt) return;
      const paths = parseDropPaths(dt);
      if (paths.length > 0) {
        e.preventDefault();
        const targetFolder = sessionStorage.getItem("hnfm-dragover-folder");
        const target = targetFolder || currentPathRef.current || "/";
        invoke("import_files", { tabId, sources: paths, targetDir: target }).then(() => {
          showToast(t("toast.imported_files", paths.length), "success");
        }).catch((e) => {
          console.error("[dnd] importFiles failed:", e);
          showToast(t("error.import_failed") || "导入失败", "error");
        });
      }
    };

    const handleDocDragLeave = (e: DragEvent) => {
      // 只在真正离开窗口时清除（relatedTarget 为 null）
      if (!e.relatedTarget) {
        setDragOverPathRef.current(null);
      }
    };

    document.addEventListener("dragover", handleDocDragOver, true);
    document.addEventListener("drop", handleDocDrop, true);
    document.addEventListener("dragleave", handleDocDragLeave, true);
    return () => {
      document.removeEventListener("dragover", handleDocDragOver, true);
      document.removeEventListener("drop", handleDocDrop, true);
      document.removeEventListener("dragleave", handleDocDragLeave, true);
    };
  }, []);

  /** 监听 hf:file-list 事件 — 后端 watcher 推送文件变化（纯事件驱动） */
  useEffect(() => {
    if (!isActive) return;

    const setup = async () => {
      const ul = await listen<WatchDelta>("hf:file-list", (event) => {
        const delta = event.payload;
        if ("Reset" in delta) {
          const mapped = delta.Reset.map(mapBackendFile);
          setFiles(mapped);
          setInaccessible(null);
          setCurrentPath(initialPathRef.current);
        } else if ("UpsertBatch" in delta) {
          setFiles((prev) => {
            const next = [...prev];
            for (const entry of delta.UpsertBatch) {
              const mapped = mapBackendFile(entry);
              const idx = next.findIndex((f) => f.path === mapped.path);
              if (idx >= 0) {
                next[idx] = mapped;
              } else {
                next.push(mapped);
              }
            }
            return next;
          });
        } else if ("Upsert" in delta) {
          setInaccessible(null);
          const entry = mapBackendFile(delta.Upsert);
          setFiles((prev) => {
            const idx = prev.findIndex((f) => f.path === entry.path);
            if (idx >= 0) {
              const next = [...prev];
              next[idx] = entry;
              return next;
            }
            return [...prev, entry];
          });
        } else if ("Remove" in delta) {
          setFiles((prev) => prev.filter((f) => f.path !== delta.Remove));
        } else if ("Rename" in delta) {
          const { from, to } = delta.Rename;
          const toName = to.split("/").pop() || to;
          setFiles((prev) => {
            const next = prev.filter((f) => f.path !== from);
            const existing = prev.find((f) => f.path === from);
            if (existing) {
              next.push({ ...existing, path: to, name: toName });
            }
            return next;
          });
        } else if ("Inaccessible" in delta) {
          const { path, ancestor, reason } = delta.Inaccessible;
          setInaccessible({ path: path.toString(), ancestor: ancestor.toString(), reason });
          setFiles([]);
        } else if ("Recovering" in delta) {
          const { path, ancestor } = delta.Recovering;
          setInaccessible({
            path: path.toString(),
            ancestor: ancestor.toString(),
            reason: `目录暂不可访问，正在 ${ancestor} 等待恢复...`,
          });
        } else if ("FatalError" in delta) {
          setInaccessible({ path: delta.FatalError.path.toString(), ancestor: '', reason: delta.FatalError.reason });
          setFiles([]);
        }
      });
      unlistenFileListRef.current = ul;
    };
    setup();

    return () => {
      if (unlistenFileListRef.current) {
        unlistenFileListRef.current();
        unlistenFileListRef.current = null;
      }
    };
  }, [isActive]); // eslint-disable-line react-hooks/exhaustive-deps

  /** 同步 initialPath（后端 nav 推送的路径）到 currentPath */
  useEffect(() => {
    if (initialPath === "app://dashboard") {
      setCurrentPath(initialPath);
      setInaccessible(null);
      return;
    }
    setCurrentPath(initialPath);
  }, [initialPath]);

  const handleNavigate = useCallback(async (file: IFile) => {
    if (file.isDirectory) {
      const now = Date.now();
      const last = lastNavRef.current;
      if (last?.path === file.path && now - last.time < 300) return;
      lastNavRef.current = { path: file.path, time: now };
      onPathChange(file.path);
    } else if (file.mime === 'inode/blockdevice' && file.isMountable) {
      const devPath = file.devicePath || file.path;
      if (file.mountedAt) {
        onPathChange(file.mountedAt);
      } else if (file.canAutoMount && onMountDevice) {
        const result = await onMountDevice(devPath);
        if (result && 'success' in result && result.success && result.mountpoint) {
          onPathChange(result.mountpoint);
        }
        // error toast handled by useDeviceActions
      } else {
        showToast(t('device.needs_auth'), 'warning');
      }
    } else if (file.mime === 'inode/blockdevice') {
      showToast(t('device.cannot_mount'), 'warning');
    } else {
      openFile(file.path);
    }
  }, [onPathChange, onMountDevice]);

  const handleRename = useCallback(async (file: IFile, newName: string) => {
    const lastSlash = file.path.lastIndexOf('/');
    const parentDir = file.path.substring(0, lastSlash);
    await renameFile(file.path, `${parentDir}/${newName}`, () => onRefresh?.());
  }, [onRefresh]);

  const handleUp = () => {
    if (currentPath && currentPath !== 'app://dashboard') {
      const parent = currentPath.substring(0, currentPath.lastIndexOf('/')) || '/';
      onPathChange(parent);
    }
  };

  const handleDropOnTarget = useCallback(
    async (draggedFiles: IFile[], targetPath: string, operation: "move" | "copy", targetDirFiles: IFile[], sourcePath: string) => {
      console.log(`[drop] handleDropOnTarget: ${draggedFiles.length} files → "${targetPath}", op=${operation}`);
      const entries = draggedFiles
        .filter((f) => f.path !== targetPath)
        .map((f) => ({ path: f.path, name: f.name, isDir: f.isDirectory }));
      if (entries.length === 0) return;

      const existingNames = targetDirFiles.map((f) => f.name);
      const conflictList = await checkConflicts(entries, targetPath);
      let renameMap: Map<string, string> | undefined;
      let conflictAction: 'skip' | 'auto-rename' = 'skip';

      if (conflictList.length > 0) {
        const result = await onConflictDialog(conflictList, targetPath, existingNames, sourcePath, operation);
        conflictAction = result.action;
        if (result.renames) renameMap = result.renames;
      }

      const conflictNames = new Set(conflictList.map((c) => c.entry.name));
      const usedNames = new Set(existingNames);

      const toProcess: { src: string; dest: string }[] = [];

      for (const entry of entries) {
        let destName = entry.name;
        if (conflictNames.has(entry.name)) {
          if (conflictAction === 'skip') continue;
          if (renameMap) {
            const renamed = renameMap.get(entry.name);
            if (!renamed || !renamed.trim()) continue;
            destName = renamed.trim();
          } else {
            const { base, ext } = splitNameExt(entry.name, entry.isDir);
            destName = generateSafeName(base, ext, usedNames, entry.isDir);
            usedNames.add(destName);
          }
        }

        const destPath = (targetPath === "/" ? "" : targetPath) + '/' + destName;
        if (destName.includes('/') || destName.includes('..')) {
          const ok = await prepareDestParent(destPath);
          if (!ok) continue;
        }
        toProcess.push({ src: entry.path, dest: destPath });
      }

      if (toProcess.length === 0) return;

      const jobId = await window.electron.startJob({
        type: operation,
        items: toProcess,
      });

      const toastId = showProgressToast(t('toast.pasting_items'), {
        total: toProcess.length,
        onCancel: () => { window.electron.cancelJob(jobId); },
      });

      const unsubProgress = window.electron.onJobProgress(jobId, (data) => {
        updateProgress(toastId, data.current);
      });

      window.electron.onJobComplete(jobId, (data) => {
        unsubProgress();

        if (data.cancelled) {
          finishToast(toastId, t('toast.operation_cancelled'), 'warning');
        } else if (data.success > 0) {
          finishToast(
            toastId,
            operation === 'copy' ? t('toast.copied_items', data.success) : t('toast.moved_items', data.success),
            'success',
          );
          if (data.fail > 0) {
            showToast(t('toast.failed_items', data.fail), 'error');
          }
        } else {
          finishToast(toastId, t('toast.failed_items', data.fail), 'error');
        }

        onRefresh?.();
      });
    },
    [onConflictDialog],
  );

  // Sort State
  const [sortBy, setSortBy] = useState<'name' | 'size' | 'date'>('name');
  const [sortOrder, setSortOrder] = useState<'asc' | 'desc'>('asc');

  // Grouping State
  const [groupingEnabled, setGroupingEnabled] = useState(true);

  // Filter AND Sort files
  const sortedFiles = useMemo(() => {
    const filtered = files.filter(f => showHiddenFiles || !f.name.startsWith('.'));
    return filtered.sort((a: IFile, b: IFile) => {
      if (groupingEnabled) {
        const groupA = getSemanticGroup(a);
        const groupB = getSemanticGroup(b);
        if (groupA !== groupB) {
          return GROUP_ORDER.indexOf(groupA) - GROUP_ORDER.indexOf(groupB);
        }
      } else {
        if (a.isDirectory !== b.isDirectory) {
          return a.isDirectory ? -1 : 1;
        }
      }

      let result = 0;
      switch (sortBy) {
      case 'name':
        result = a.name.localeCompare(b.name);
        break;
      case 'size':
        result = a.size - b.size;
        break;
      case 'date':
        result = a.mtime.getTime() - b.mtime.getTime();
        break;
      }
      return sortOrder === 'asc' ? result : -result;
    });
  }, [files, showHiddenFiles, sortBy, sortOrder, groupingEnabled]);
    // Selection State
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [lastSelectedPath, setLastSelectedPath] = useState<string | null>(null);
  const [selectionMode, setSelectionMode] = useState<string | null>(null);
  const [modifiers, setModifiers] = useState({ ctrl: false, shift: false });
  const [suppressClickHint, setSuppressClickHint] = useState(false);
  const mouseDownRef = useRef(false);

  const handleSelectionModeChange = useCallback((mode: "replace" | "union" | "intersection" | "difference" | null) => {
    setSelectionMode(mode);
    if (mode !== null) {
      setSuppressClickHint(true);
    }
  }, []);

  const handleHoverFile = useCallback((file: IFile | null) => {
    setHoveredFile(file);
  }, []);

  const selectionHint = useMemo(() => {
    if (selectionMode) {
      const labelMap: Record<string, string> = {
        replace: t('selection.box_replace'),
        union: t('selection.box_union'),
        intersection: t('selection.box_intersection'),
        difference: t('selection.box_difference'),
      };
      return labelMap[selectionMode] || selectionMode;
    }
    if (suppressClickHint) return null;
    if (!modifiers.ctrl && !modifiers.shift) return null;
    if (modifiers.ctrl && modifiers.shift) return t('selection.click_range_add');
    if (modifiers.ctrl) return t('selection.click_add_remove');
    return t('selection.click_range');
  }, [selectionMode, modifiers, suppressClickHint]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.repeat) return;
      if (e.key === "Control" || e.key === "Shift") {
        setModifiers((prev) => ({
          ctrl: e.key === "Control" ? true : prev.ctrl,
          shift: e.key === "Shift" ? true : prev.shift,
        }));
        if (mouseDownRef.current) {
          setSuppressClickHint(true);
        } else {
          setSuppressClickHint(false);
        }
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Control" || e.key === "Shift") {
        setModifiers((prev) => {
          const next = {
            ctrl: e.key === "Control" ? false : prev.ctrl,
            shift: e.key === "Shift" ? false : prev.shift,
          };
          if (!next.ctrl && !next.shift) {
            setSuppressClickHint(false);
          }
          return next;
        });
      }
    };
    const onMouseDown = () => {
      mouseDownRef.current = true;
    };
    const onMouseUp = () => {
      mouseDownRef.current = false;
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, []);

  // Clear selection on path change
  const prevPathRef = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (prevPathRef.current === undefined) {
      prevPathRef.current = initialPath;
      return;
    }
    if (prevPathRef.current !== initialPath) {
      setSelectedFiles(new Set());
      setLastSelectedPath(null);
      prevPathRef.current = initialPath;
    }
  }, [initialPath]);

  const handleSelect = (file: IFile, toggle: boolean, range: boolean) => {
    const newSelection = new Set(toggle ? selectedFiles : []);

    if (range && lastSelectedPath) {
      const start = sortedFiles.findIndex(f => f.path === lastSelectedPath);
      const end = sortedFiles.findIndex(f => f.path === file.path);
      if (start !== -1 && end !== -1) {
        const low = Math.min(start, end);
        const high = Math.max(start, end);
        for (let i = low; i <= high; i++) {
          newSelection.add(sortedFiles[i].path);
        }
      } else {
        newSelection.add(file.path);
      }
    } else if (toggle) {
      if (selectedFiles.has(file.path)) {
        newSelection.delete(file.path);
      } else {
        newSelection.add(file.path);
        setLastSelectedPath(file.path);
      }
    } else {
      newSelection.add(file.path);
      setLastSelectedPath(file.path);
    }

    setSelectedFiles(newSelection);
  };

  const executePasteAction = useCallback(async () => {
    if (clipboard && clipboard.files.length > 0) {
      const existingNames = files.map((f) => f.name);
      await pasteFiles(
        clipboard.files,
        clipboard.operation,
        currentPath,
        existingNames,
        clipboard.operation === 'cut' ? clearClipboard : undefined,
        () => onRefresh?.(),
        (conflicts) => onConflictDialog(conflicts, currentPath, existingNames),
      );
    }
  }, [clipboard, files, currentPath, clearClipboard, onRefresh, onConflictDialog]);

  // Keyboard Shortcuts
  useEffect(() => {
    if (!isActive) return;

    const handleKeyDown = async (e: KeyboardEvent) => {
      // Don't handle shortcuts when focus is on an input/textarea, or when dialogs/context-menus are open
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;
      if (document.querySelector('md-dialog[open], .context-menu, [role="dialog"]')) return;

      if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
        e.preventDefault();
        const allPaths = new Set(sortedFiles.map(f => f.path));
        setSelectedFiles(allPaths);
        return;
      }

      if (e.key === 'F5') {
        e.preventDefault();
        onRefresh?.();
        return;
      }

      if (e.key === 'Delete') {
        e.preventDefault();
        if (selectedFiles.size > 0) {
          if (window.confirm(t('dialog.delete.confirm', selectedFiles.size))) {
            await trashFiles(Array.from(selectedFiles), () => onRefresh?.());
          }
        }
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === 'c') {
        if (selectedFiles.size > 0) {
          const filesToCopy = sortedFiles.filter(f => selectedFiles.has(f.path));
          copy(filesToCopy);
          copyToClipboard(selectedFiles.size);
        }
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === 'x') {
        if (selectedFiles.size > 0) {
          const filesToCut = sortedFiles.filter(f => selectedFiles.has(f.path));
          cut(filesToCut);
          cutToClipboard(selectedFiles.size);
        }
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === 'v') {
        e.preventDefault();
        executePasteAction();
        return;
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isActive, sortedFiles, selectedFiles, currentPath, onRefresh, clipboard]);

  const handleBackgroundContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
        
    const currentFolderAsFile: IFile = {
      name: currentPath.split('/').pop() || currentPath,
      path: currentPath,
      isDirectory: true,
      size: 0,
      mtime: new Date(),
      mime: null
    };

    const customItems = [
      {
        label: t('context_menu.refresh'),
        icon: 'refresh',
        action: () => onRefresh?.()
      },
      { label: '', divider: true, action: () => {} },
      {
        label: t('context_menu.new_folder'),
        icon: 'create_new_folder',
        action: () => {
          const existingNames = files.map(f => f.name);
          void (async () => {
            const name = await onCreateDialog('folder', t('dialog.create.default_folder'), existingNames);
            if (name) {
              await createDirectory(currentPath + '/' + name, () => onRefresh?.());
            }
          })();
        },
      },
      {
        label: t('context_menu.new_file'),
        icon: 'note_add',
        action: () => {
          const existingNames = files.map(f => f.name);
          void (async () => {
            const name = await onCreateDialog('file', t('dialog.create.default_file'), existingNames);
            if (name) {
              await createFile(currentPath + '/' + name, () => onRefresh?.());
            }
          })();
        },
      },
      ...(clipboard && clipboard.files.length > 0 ? [
        { label: '', divider: true, action: () => {} } as ContextMenuItem,
        {
          label: t('context_menu.paste'),
          icon: 'content_paste',
          action: () => executePasteAction(),
        } as ContextMenuItem,
        { label: '', divider: true, action: () => {} } as ContextMenuItem,
      ] : []),
      {
        label: t('context_menu.open_terminal'),
        icon: 'terminal',
        action: () => onOpenTerminalAt(currentPath)
      },
      {
        label: t('context_menu.open_with'),
        icon: 'apps',
        action: () => {
          onOpenWithFile(currentFolderAsFile);
        }
      },
      { label: '', divider: true, action: () => {} },
      {
        label: t('context_menu.properties'),
        icon: 'info',
        action: () => {
          onPropertiesFile(currentFolderAsFile);
        }
      }
    ];

    onContextMenu(e, null);
    onBgMenuItems(customItems);
  }, [currentPath, files, clipboard, onCreateDialog, onRefresh, executePasteAction, onOpenTerminalAt, onOpenWithFile, onPropertiesFile, onContextMenu, onBgMenuItems]);

  // ── Stable callback wrappers for FileList (ref pattern to prevent unnecessary re-renders) ──
  const handleSelectRef = useRef(handleSelect);
  // eslint-disable-next-line react-hooks/refs -- keep ref in sync with latest handler
  handleSelectRef.current = handleSelect;
  const selectedFilesForFileListRef = useRef(selectedFiles);
  // eslint-disable-next-line react-hooks/refs -- keep ref in sync during render for stable callbacks
  selectedFilesForFileListRef.current = selectedFiles;
  const filesForFileListRef = useRef(files);
  // eslint-disable-next-line react-hooks/refs -- keep ref in sync during render for stable callbacks
  filesForFileListRef.current = files;
  const handleDropOnTargetRef = useRef(handleDropOnTarget);
  // eslint-disable-next-line react-hooks/refs -- keep ref in sync with latest handler
  handleDropOnTargetRef.current = handleDropOnTarget;

  const handleFileContextMenu = useCallback((e: React.MouseEvent, file: IFile) => {
    if (file && !selectedFilesForFileListRef.current.has(file.path)) {
      handleSelectRef.current(file, false, false);
    }
    onContextMenu(e, file);
  }, [onContextMenu]);

  const handleDeselectAll = useCallback(() => {
    setSelectedFiles(new Set());
  }, []);

  /**
   * dnd-kit 拖放结束回调。
   *
   * - 拖放到文件夹/背景：执行 move/copy 操作
   * - 拖放到外部（over 为 null）：调用 startDrag 触发原生拖放
   */
  const handleDragEnd = useCallback(
    (event: DragEndEvent, shiftKey: boolean) => {
      const { active, over } = event;

      const dragData = active.data.current as DndDragData | undefined;
      if (!dragData || dragData.files.length === 0) return;

      // 拖放到外部（没有 over 目标）→ 调用 startDrag 触发原生拖放
      if (!over) {
        const paths = dragData.files.map((f) => f.path);
        if (isTauri()) {
          import("../utils/drag").then(({ startDrag }) => {
            startDrag({ item: paths }, (payload) => {
              console.log("[dnd] drag result:", payload.result);
            }).catch((e) => {
              console.error("[dnd] startDrag failed:", e);
            });
          });
        } else {
          console.log("[mock] startDrag:", paths);
        }
        return;
      }

      const overData = over.data.current as { path?: string; isDirectory?: boolean; isBackground?: boolean } | undefined;
      if (!overData?.path) return;

      // 不允许拖到自身
      if (dragData.files.some((f) => f.path === overData.path)) return;

      const targetPath = overData.path;
      const operation = shiftKey ? "copy" : "move";

      handleDropOnTargetRef.current(
        dragData.files,
        targetPath,
        operation,
        filesForFileListRef.current,
        dragData.sourcePath,
      );
    },
    []
  );

  const stableHandleSelect = useCallback((file: IFile, toggle: boolean, range: boolean) => {
    handleSelectRef.current(file, toggle, range);
  }, []);

  /** 拖拽离开窗口 → 触发 Tauri 原生拖放（App→外部应用） */
  const handleDragLeaveWindow = useCallback((draggedFiles: IFile[]) => {
    const paths = draggedFiles.map((f) => f.path);
    if (isTauri()) {
      import("../utils/drag").then(({ startDrag }) => {
        startDrag({ item: paths }, (payload) => {
          console.log("[dnd] drag result:", payload.result);
        }).catch((e) => {
          console.error("[dnd] startDrag failed:", e);
        });
      });
    } else {
      console.log("[mock] startDrag (leave window):", paths);
    }
  }, []);

  /** 拖拽开始 → emit 数据到其他窗口（跨窗口拖放） */
  const handleDragStart = useCallback((info: { files: IFile[]; sourcePath: string }) => {
    if (isTauri()) {
      import("@tauri-apps/api/event").then(({ emit }) => {
        emit("dnd:drag-start", {
          files: info.files.map((f) => ({ path: f.path, name: f.name, isDirectory: f.isDirectory })),
          sourcePath: info.sourcePath,
        }).catch((e: unknown) => {
          console.error("[dnd] emit drag-start failed:", e);
        });
      });
    }
  }, []);

  return (
    <FileDndProvider onDragStart={handleDragStart} onDragEnd={handleDragEnd} onDragLeaveWindow={handleDragLeaveWindow}>
      <div style={{ display: isActive ? 'flex' : 'none', flexDirection: 'column', flex: 1, height: '100%', overflow: 'hidden' }}>
        {/* Top Bar */}
        {(currentPath !== 'app://dashboard') && (
          <div style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '16px', padding: '8px 24px 0' }}>
            <IconButton onClick={handleUp} variant="standard">
              <Icon name="arrow_upward" />
            </IconButton>
            <div style={{ flex: 1, overflow: 'hidden' }}>
            <Omnibar
              currentPath={currentPath}
              breadcrumbs={breadcrumbs}
              onNavigate={(p: string) => onPathChange(p)}
              onSearch={handleSearch}
            />
            </div>
            <div style={{ display: 'flex', gap: '4px' }}>
              <IconButton
                variant={groupingEnabled ? 'filled' : 'standard'}
                onClick={() => setGroupingEnabled(!groupingEnabled)}
                title={t('sort.toggle_grouping')}
              >
                <Icon name="view_agenda" />
              </IconButton>
              <div style={{ width: '1px', background: 'var(--md-sys-color-outline-variant)', margin: '0 4px' }} />
              <IconButton
                variant={sortBy === 'name' ? 'filled' : 'standard'}
                onClick={() => {
                  if (sortBy === 'name') setSortOrder(prev => prev === 'asc' ? 'desc' : 'asc');
                  else { setSortBy('name'); setSortOrder('asc'); }
                }}
                title={t('sort.by_name')}
              >
                <Icon name="sort_by_alpha" />
              </IconButton>
              <IconButton
                variant={sortBy === 'date' ? 'filled' : 'standard'}
                onClick={() => {
                  if (sortBy === 'date') setSortOrder(prev => prev === 'asc' ? 'desc' : 'asc');
                  else { setSortBy('date'); setSortOrder('desc'); }
                }}
                title={t('sort.by_date')}
              >
                <Icon name="calendar_today" />
              </IconButton>
            </div>
          </div>
        )}

        {currentPath === 'app://dashboard' ? (
          <div style={{ display: 'flex', flexDirection: 'column', flex: 1, height: '100%', overflow: 'hidden' }}>
            <Dashboard onNavigate={(p: string) => onPathChange(p)} />
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', flex: 1, overflow: 'hidden' }}>
            <div style={{ flex: 1, overflow: 'hidden' }}>
              {inaccessible ? (
                <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '100%', gap: '16px', color: 'var(--md-sys-color-on-surface-variant)' }}>
                  <Icon name="lock" size={48} />
                  <div style={{ textAlign: 'center' }}>
                    <h3 style={{ margin: '0 0 8px', fontSize: '18px', fontWeight: 500 }}>{inaccessible.path}</h3>
                    <p style={{ margin: 0, fontSize: '14px', opacity: 0.8 }}>{inaccessible.reason}</p>
                  </div>
                  <div style={{ display: 'flex', gap: '8px' }}>
                    <IconButton onClick={() => { setInaccessible(null); onPathChange(inaccessible.ancestor || '/'); }} variant="filled" title={t('breadcrumbs.go_to_root') || '后退'}>
                      <Icon name="arrow_back" />
                    </IconButton>
                    <IconButton onClick={() => onRefresh?.()} variant="standard" title={t('context_menu.refresh') || '刷新'}>
                      <Icon name="refresh" />
                    </IconButton>
                  </div>
                </div>
              ) : (
                <FileList
                  files={sortedFiles}
                  selectedFiles={selectedFiles}
                  onSelect={stableHandleSelect}
                  onNavigate={handleNavigate}
                  onRename={handleRename}
                  onContextMenu={handleFileContextMenu}
                  onBackgroundContextMenu={handleBackgroundContextMenu}
                  onDeselectAll={handleDeselectAll}
                  onSetSelected={setSelectedFiles}
                  onSelectionModeChange={handleSelectionModeChange}
                  onHoverFile={handleHoverFile}
                  currentPath={currentPath}
                  iconSize={iconSize}
                  viewMode={viewMode}
                  filledIcons={filledIcons}
                  groupingEnabled={groupingEnabled}
                  marqueeEnabled={marqueeEnabled}
                />
              )}
            </div>
          </div>
        )}

        {currentPath !== 'app://dashboard' && (
          <StatusBar totalItems={files.length} selectedCount={selectedFiles.size} selectionHint={selectionHint} hoveredFile={hoveredFile} />
        )}
      </div>
    </FileDndProvider>
  );
}
