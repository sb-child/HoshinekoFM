/**
 * DnD（拖放）封装 —— 基于 @dnd-kit/core，统一内部/外部拖放逻辑。
 *
 * 内部拖放：useDraggable + useDroppable（Pointer Events，不触发系统拖放）
 * 外部拖入：onDragDropEvent（Tauri 原生）/ HTML5 dragover/drop（浏览器）
 * 内部拖出：startDrag（Tauri 原生，已实现）
 */

import React, {
  useCallback,
  useContext,
  useRef,
  useState,
  useEffect,
  type ReactNode,
} from "react";
import {
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  pointerWithin,
  useDraggable,
  useDroppable,
  DragOverlay,
  type DragEndEvent,
  type DragStartEvent,
  type DragOverEvent,
} from "@dnd-kit/core";
import type { IFile } from "../types/files";

// ─── 类型 ────────────────────────────────────────────────────────────

/** 拖拽时传递的数据 */
export interface DndDragData {
  /** 拖拽的文件列表 */
  files: IFile[];
  /** 源目录路径 */
  sourcePath: string;
  /** 操作类型（拖拽结束时由 Shift 键决定） */
  operation: "move" | "copy";
}

/** 拖拽开始事件的简化数据 */
export interface DndDragStartInfo {
  files: IFile[];
  sourcePath: string;
  operation: "move" | "copy";
}

// ─── DragOver Context ────────────────────────────────────────────────

/**
 * 拖拽悬停目标 context —— 集中管理 drag-over 状态。
 *
 * 替代 useDroppable 的 isOver（后者与 useDraggable 同元素冲突）。
 * 数据来源：
 *   - 内部拖放：DndContext onDragOver → over.data.current.path
 *   - 外部拖入（浏览器）：HTML5 dragover → elementFromPoint → data-droppable-id
 *   - 外部拖入（Tauri）：onDragDropEvent over → elementFromPoint → data-droppable-id
 */
interface DragOverContextValue {
  /** 当前悬停的文件夹路径，null 表示没有悬停在文件夹上 */
  dragOverPath: string | null;
  setDragOverPath: (path: string | null) => void;
}

const DragOverContext = React.createContext<DragOverContextValue>({
  dragOverPath: null,
  setDragOverPath: () => {},
});

/** 供 Provider 使用的 state hook */
export function useDragOverState() {
  const [dragOverPath, setDragOverPath] = useState<string | null>(null);
  return { dragOverPath, setDragOverPath };
}

/**
 * DragOver Provider —— 提升到 App 级别，供 FileDndProvider 和 onDragDropEvent 共享。
 */
export function DragOverProvider({ children }: { children: ReactNode }) {
  const ctx = useDragOverState();
  return React.createElement(
    DragOverContext.Provider,
    { value: ctx },
    children,
  );
}

/** 子组件读取当前 drag-over 状态 */
export function useDragOver(): string | null {
  return useContext(DragOverContext).dragOverPath;
}

/** 外部更新 drag-over 状态（用于 Tauri onDragDropEvent / 浏览器 HTML5 dragover） */
export function useSetDragOver(): (path: string | null) => void {
  return useContext(DragOverContext).setDragOverPath;
}

// ─── Hooks ───────────────────────────────────────────────────────────

/**
 * 文件项可拖拽 hook。
 *
 * @param file - 当前文件
 * @param selectedFiles - 选中的文件路径集合
 * @param allFiles - 当前目录所有文件（用于获取选中文件的完整信息）
 */
export function useFileDraggable(
  file: IFile,
  selectedFiles: Set<string>,
  allFiles: IFile[],
) {
  const filesToDrag = selectedFiles.has(file.path)
    ? allFiles.filter((f) => selectedFiles.has(f.path))
    : [file];

  return useDraggable({
    id: `file:${file.path}`,
    data: {
      files: filesToDrag,
      sourcePath: file.path.substring(0, file.path.lastIndexOf("/")),
      operation: "move" as const,
    } satisfies DndDragData,
  });
}

/**
 * 文件夹可放置 hook —— 仅注册 droppable DOM 节点供 collisionDetection 测量。
 *
 * 拖拽高亮由 DragOverContext 管理（不依赖此 hook 的 isOver）。
 *
 * @param file - 文件夹文件对象
 */
export function useFolderDroppable(file: IFile) {
  return useDroppable({
    id: `folder:${file.path}`,
    data: { path: file.path, isDirectory: true },
  });
}

// ─── Provider ────────────────────────────────────────────────────────

interface FileDndProviderProps {
  children: ReactNode;
  /** 拖拽开始回调 */
  onDragStart?: (info: DndDragStartInfo) => void;
  /** 拖拽结束回调 */
  onDragEnd: (event: DragEndEvent, shiftKey: boolean) => void;
  /** 拖拽悬停目标变化回调 */
  onDragOver?: (path: string | null) => void;
  /** 拖拽离开窗口回调（用于触发原生拖放到外部应用） */
  onDragLeaveWindow?: (files: IFile[]) => void;
  /** 拖拽预览内容（简单文本，后续 Rust 接管） */
  dragPreview?: ReactNode;
}

/**
 * 文件拖放 Provider —— 封装 DndContext + Sensors + DragOverlay + DragOverContext。
 */
export function FileDndProvider({
  children,
  onDragStart,
  onDragEnd,
  onDragOver,
  onDragLeaveWindow,
  dragPreview,
}: FileDndProviderProps) {
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 5, // 拖动 5px 后才激活，避免误触
      },
    }),
  );

  // ── Shift 键跟踪 ──
  const shiftKeyRef = useRef(false);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Shift") shiftKeyRef.current = true;
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Shift") shiftKeyRef.current = false;
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, []);

  // ── DragOver 状态（从 App 级别 DragOverContext 读取 setter） ──
  const { setDragOverPath } = useContext(DragOverContext);
  const onDragOverRef = useRef(onDragOver);
  onDragOverRef.current = onDragOver;

  // ── 拖拽中状态（用于指针越界检测） ──
  const isDraggingRef = useRef(false);
  const activeDragDataRef = useRef<DndDragData | null>(null);
  const onDragLeaveWindowRef = useRef(onDragLeaveWindow);
  onDragLeaveWindowRef.current = onDragLeaveWindow;
  const leaveCheckIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // ── 指针坐标跟踪 ──
  useEffect(() => {
    const onPointerMove = (e: PointerEvent) => {
      lastPointerRef.current = { clientX: e.clientX, clientY: e.clientY };
    };
    document.addEventListener("pointermove", onPointerMove, true);
    return () => {
      document.removeEventListener("pointermove", onPointerMove, true);
      // 清理 interval
      if (leaveCheckIntervalRef.current) {
        clearInterval(leaveCheckIntervalRef.current);
        leaveCheckIntervalRef.current = null;
      }
    };
  }, []);

  // 最近的指针坐标
  const lastPointerRef = useRef({ clientX: 0, clientY: 0 });

  const handleDragStart = useCallback(
    (event: DragStartEvent) => {
      const data = event.active.data.current as DndDragData | undefined;
      if (!data) return;

      isDraggingRef.current = true;
      activeDragDataRef.current = data;

      // 记录初始指针位置
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const evt = (event as any).activatorEvent as PointerEvent | undefined;
      if (evt) {
        lastPointerRef.current = { clientX: evt.clientX, clientY: evt.clientY };
      }

      // 启动指针越界检测 interval
      if (onDragLeaveWindowRef.current && !leaveCheckIntervalRef.current) {
        leaveCheckIntervalRef.current = setInterval(() => {
          if (!isDraggingRef.current) return;
          const { clientX: x, clientY: y } = lastPointerRef.current;
          const W = window.innerWidth;
          const H = window.innerHeight;
          if (x < 0 || y < 0 || x > W || y > H) {
            const dragData = activeDragDataRef.current;
            if (dragData && dragData.files.length > 0) {
              onDragLeaveWindowRef.current?.(dragData.files);
            }
            // 通知 dnd-kit 拖拽已结束（GTK 原生拖放已接管）
            // dnd-kit 的 PointerSensor 监听 document 上的 pointerup
            isDraggingRef.current = false;
            activeDragDataRef.current = null;
            document.dispatchEvent(
              new PointerEvent("pointerup", { bubbles: true, cancelable: true }),
            );
            if (leaveCheckIntervalRef.current) {
              clearInterval(leaveCheckIntervalRef.current);
              leaveCheckIntervalRef.current = null;
            }
          }
        }, 50);
      }

      onDragStart?.({
        files: data.files,
        sourcePath: data.sourcePath,
        operation: data.operation,
      });
    },
    [onDragStart],
  );

  const handleDragOver = useCallback(
    (event: DragOverEvent) => {
      const overData = event.over?.data.current as
        | { path?: string; isDirectory?: boolean }
        | undefined;
      const path = overData?.isDirectory ? overData.path ?? null : null;
      setDragOverPath(path);
      onDragOverRef.current?.(path);
    },
    [setDragOverPath],
  );

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      isDraggingRef.current = false;
      activeDragDataRef.current = null;
      if (leaveCheckIntervalRef.current) {
        clearInterval(leaveCheckIntervalRef.current);
        leaveCheckIntervalRef.current = null;
      }
      setDragOverPath(null);
      onDragOverRef.current?.(null);
      onDragEnd(event, shiftKeyRef.current);
    },
    [onDragEnd, setDragOverPath],
  );

  const handleDragCancel = useCallback(() => {
    isDraggingRef.current = false;
    activeDragDataRef.current = null;
    if (leaveCheckIntervalRef.current) {
      clearInterval(leaveCheckIntervalRef.current);
      leaveCheckIntervalRef.current = null;
    }
    setDragOverPath(null);
    onDragOverRef.current?.(null);
  }, [setDragOverPath]);

  return React.createElement(
    DndContext,
    {
      sensors,
      collisionDetection: pointerWithin,
      onDragStart: handleDragStart,
      onDragOver: handleDragOver,
      onDragEnd: handleDragEnd,
      onDragCancel: handleDragCancel,
    },
    children,
    React.createElement(DragOverlay, null, dragPreview),
  );
}

// ─── 工具函数 ────────────────────────────────────────────────────────

/**
 * 从 DataTransfer 的 text/uri-list 和 text/plain 中提取文件路径。
 *
 * 用于外部拖放（Nautilus、VSCode 等 → App）。
 * 浏览器对外部拖放通常提供 text/uri-list（Nautilus）或 text/plain（VSCode），
 * `e.dataTransfer.files` 在浏览器中不暴露路径。
 */
export function parseDropPaths(dt: DataTransfer): string[] {
  // 1) text/uri-list（Nautilus、跨窗口拖放）
  const uriList = dt.getData("text/uri-list");
  if (uriList) {
    const paths = uriList
      .split(/\r?\n/)
      .filter(Boolean)
      .map((uri) => {
        try {
          return decodeURI(new URL(uri).pathname);
        } catch {
          return uri.replace(/^file:\/\//, "");
        }
      })
      .filter(Boolean);
    if (paths.length > 0) return paths;
  }

  // 2) text/plain（VSCode、部分编辑器拖出只提供文本行）
  const text = dt.getData("text/plain");
  if (text) {
    const paths = text
      .split(/\r?\n/)
      .filter(Boolean)
      .filter((line) => /^\//.test(line.trim())) // 只取看起来像绝对路径的行
      .map((l) => l.trim());
    if (paths.length > 0) return paths;
  }

  return [];
}

/**
 * 从路径列表构造最小 IFile 对象（用于外部拖放）。
 */
export function pathsToFiles(paths: string[]): IFile[] {
  return paths.map((p) => ({
    name: p.split("/").pop() || p,
    path: p,
    isDirectory: false,
    size: 0,
    mtime: new Date(),
    mime: null,
  }));
}
