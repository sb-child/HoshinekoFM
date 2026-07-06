import { useState, useCallback, useRef, useEffect, memo } from "react";
import type { IFile } from "../types/files";
import "./FileList.css";
import { AutoSizer } from "react-virtualized-auto-sizer";
import { List, useListRef } from "react-window";
import { useDrag } from "../contexts/DragContext";
import {
  type ItemBox,
  DOUBLE_CLICK_THRESHOLD,
  flattenItems,
  LIST_ROW_HEIGHT,
  GRID_ROW_HEIGHT,
  HEADER_HEIGHT,
  computeItemBoxes,
} from "./FileList/utils";
import { Row, type RowData } from "./FileList/Row";
import { useRubberBandSelection } from "../hooks/useRubberBandSelection";

interface FileListProps {
  files: IFile[];
  selectedFiles: Set<string>;
  onSelect: (file: IFile, toggle: boolean, range: boolean) => void;
  onNavigate: (file: IFile) => void;
  onRename?: (file: IFile, newName: string) => void;
  onContextMenu?: (e: React.MouseEvent, file: IFile) => void;
  onBackgroundContextMenu?: (e: React.MouseEvent) => void;
  onDeselectAll?: () => void;
  onDropOnFolder?: (
    files: IFile[],
    targetPath: string,
    operation: "move" | "copy",
  ) => void;
  onSetSelected?: (paths: Set<string>) => void;
  onSelectionModeChange?: (
    mode: "replace" | "union" | "intersection" | "difference" | null,
  ) => void;
  onHoverFile?: (file: IFile | null) => void;
  viewMode: "grid" | "list";
  iconSize: number;
  filledIcons: boolean;
  groupingEnabled?: boolean;
  currentPath?: string;
  scrollToFileName?: string;
  onScrollToComplete?: () => void;
  marqueeEnabled: boolean;
}

/** Internal drag session — stored in a ref to avoid React re-renders during
 *  frequent mousemove events. */
interface DragSession {
  files: IFile[];
  sourcePath: string;
  startX: number;
  startY: number;
}

/** Minimum mouse movement (px) before a drag is initiated. */
const DRAG_THRESHOLD = 5;

/** Offset of the drag preview from the cursor (top-left). */
const DRAG_PREVIEW_OFFSET_X = 16;
const DRAG_PREVIEW_OFFSET_Y = 16;

// --- Main component ---

const FileListComponent: React.FC<FileListProps> = ({
  files,
  selectedFiles,
  onSelect,
  onNavigate,
  onRename,
  onContextMenu,
  onBackgroundContextMenu,
  onDeselectAll,
  onDropOnFolder,
  onSetSelected,
  onSelectionModeChange,
  onHoverFile,
  viewMode,
  iconSize,
  filledIcons,
  groupingEnabled = false,
  currentPath,
  scrollToFileName,
  onScrollToComplete,
  marqueeEnabled,
}) => {
  const [failedImages, setFailedImages] = useState<Set<string>>(new Set());
  const [renamingPath, setRenamingPath] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [dragOverPath, setDragOverPath] = useState<string | null>(null);

  // --- Drag simulation state (mouse-based, replaces HTML5 DnD) ---
  const [isDragging, setIsDragging] = useState(false);
  // Ref mirror — event handlers need the current value without closure staleness
  const isDraggingRef = useRef(false);
  const dragSessionRef = useRef<DragSession | null>(null);
  const dragPendingRef = useRef(false); // mousedown but not yet moved past threshold
  const dragPreviewElRef = useRef<HTMLDivElement | null>(null);
  const lastHoveredFolderRef = useRef<IFile | null>(null);
  const rafRef = useRef(0);

  const lastClickRef = useRef<{ path: string; time: number } | null>(null);
  const renameTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const containerRef = useRef<HTMLDivElement>(null);
  const listImperativeRef = useListRef(null);

  const itemBoxesRef = useRef<ItemBox[]>([]);

  const {
    isSelectingRef,
    didSelectRef,
    selectionBox,
    handleBackgroundMouseDown,
  } = useRubberBandSelection(
    containerRef,
    listImperativeRef,
    itemBoxesRef,
    selectedFiles,
    onSetSelected,
    onSelectionModeChange,
  );

  const { startDrag, endDrag } = useDrag();

  useEffect(() => {
    return () => {
      if (renameTimeoutRef.current !== null) {
        clearTimeout(renameTimeoutRef.current);
      }
    };
  }, []);

  // Reset failed images when directory changes
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- clearing stale failed images on dir change
    setFailedImages(new Set());
  }, [currentPath]);

  // Scroll to file when scrollToFileName changes and files are loaded
  const prevScrollTargetRef = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!scrollToFileName) {
      prevScrollTargetRef.current = undefined;
      return;
    }

    const scrollKey = scrollToFileName + "|" + (currentPath || "");
    if (scrollKey === prevScrollTargetRef.current) return;
    prevScrollTargetRef.current = scrollKey;

    const idx = files.findIndex((f) => f.name === scrollToFileName);
    if (idx === -1) return;

    const listEl = listImperativeRef.current;
    if (!listEl) return;

    const containerWidth = listEl.element?.parentElement?.clientWidth ?? 600;
    const columns =
      viewMode === "grid"
        ? Math.max(1, Math.floor((containerWidth + 8) / (iconSize + 40)))
        : 0;
    const items = flattenItems(files, groupingEnabled, viewMode, columns);
    let flattenedIdx = -1;
    let foundFileIdx = 0;
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.kind === "file" || item.kind === "grid-row") {
        const fileList = item.kind === "file" ? [item.file] : item.files;
        for (let fi = 0; fi < fileList.length; fi++) {
          if (foundFileIdx === idx) {
            flattenedIdx = i;
            break;
          }
          foundFileIdx++;
        }
        if (flattenedIdx !== -1) break;
      }
    }

    if (flattenedIdx !== -1) {
      listEl.scrollToRow({ index: flattenedIdx, align: "smart" });
    }

    const targetFile = files[idx];
    if (targetFile && onSelect) {
      onSelect(targetFile, false, false);
      onScrollToComplete?.();
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    scrollToFileName,
    files,
    viewMode,
    iconSize,
    groupingEnabled,
    onSelect,
    currentPath,
    onScrollToComplete,
  ]);  

  const handleImageError = useCallback((path: string) => {
    setFailedImages((prev) => {
      if (prev.has(path)) return prev;
      const next = new Set(prev);
      next.add(path);
      return next;
    });
  }, []);

  // --- Item click (unchanged, but no longer handles dragstart) ---
  const handleItemClick = useCallback(
    (e: React.MouseEvent, file: IFile) => {
      // Ignore if we were dragging — mouseup handles that
      if (isDraggingRef.current || dragPendingRef.current) return;

      document.activeElement?.blur();
      if (renamingPath) return;
      if (isSelectingRef.current) return;
      if (didSelectRef.current) {
        didSelectRef.current = false;
        return;
      }

      const isModifier = e.ctrlKey || e.metaKey;
      const isRange = e.shiftKey;
      if (isModifier || isRange) {
        lastClickRef.current = { path: file.path, time: Date.now() };
        onSelect(file, isModifier, isRange);
        return;
      }

      const now = Date.now();
      const last = lastClickRef.current;

      if (last?.path === file.path) {
        if (now - last.time < DOUBLE_CLICK_THRESHOLD) {
          lastClickRef.current = null;
          onNavigate(file);
          return;
        }
        lastClickRef.current = null;
        renameTimeoutRef.current = setTimeout(() => {
          renameTimeoutRef.current = null;
          setRenamingPath(file.path);
          setRenameValue(file.name);
        }, 0);
        return;
      }

      onSelect(file, false, false);
      lastClickRef.current = { path: file.path, time: now };
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [onSelect, onNavigate, renamingPath],
  );

  const handleItemDoubleClick = useCallback(
    (file: IFile) => {
      if (renameTimeoutRef.current !== null) {
        clearTimeout(renameTimeoutRef.current);
        renameTimeoutRef.current = null;
      }
      onNavigate(file);
    },
    [onNavigate],
  );

  const handleRenameInputChange = useCallback((value: string) => {
    setRenameValue(value);
  }, []);

  const handleRenameSubmit = useCallback(() => {
    if (!renamingPath) return;
    const file = files.find((f) => f.path === renamingPath);
    if (file && renameValue && renameValue !== file.name) {
      onRename?.(file, renameValue);
    }
    setRenamingPath(null);
    setRenameValue("");
  }, [renamingPath, renameValue, files, onRename]);

  const handleRenameCancel = useCallback(() => {
    setRenamingPath(null);
    setRenameValue("");
  }, []);

  // ═══════════════════════════════════════════════════════════════
  //  Mouse-based drag simulation (replaces HTML5 DnD)
  // ═══════════════════════════════════════════════════════════════

  /** Create the floating drag preview element. */
  const createDragPreview = useCallback(
    (dfs: IFile[], x: number, y: number) => {
      const el = document.createElement("div");
      el.className = "drag-preview";
      el.style.cssText = `
        position:fixed; pointer-events:none; z-index:999999;
        left:${x + DRAG_PREVIEW_OFFSET_X}px;
        top:${y + DRAG_PREVIEW_OFFSET_Y}px;
        background:var(--md-sys-color-surface-container-high,#2a2a2a);
        color:var(--md-sys-color-on-surface,#fff);
        border-radius:8px; padding:6px 12px;
        font-size:14px; line-height:1.3;
        box-shadow:0 4px 12px rgba(0,0,0,0.4);
        display:flex; align-items:center; gap:6px;
        white-space:nowrap; user-select:none;
      `;
      if (dfs.length === 1) {
        el.textContent = dfs[0].name;
      } else {
        el.textContent = `${dfs.length} 个项目`;
      }
      document.body.appendChild(el);
      dragPreviewElRef.current = el;
    },
    [],
  );

  /** Find a folder file-list-item at the given coordinates. */
  const findFolderAtPoint = useCallback(
    (x: number, y: number): IFile | null => {
      const target = document.elementFromPoint(x, y);
      if (!target) return null;
      const row = target.closest("[data-file-path]") as HTMLElement | null;
      if (!row) return null;
      const filePath = row.getAttribute("data-file-path");
      const isDir = row.getAttribute("data-is-dir") === "1";
      if (!isDir || !filePath) return null;
      // Look up the IFile from our file list
      // We search by path since file objects may be different references
      return files.find((f) => f.path === filePath) ?? null;
    },
    [files],
  );

  /** Clean up all drag state (called on drop, cancel, or native handoff). */
  const cleanupDrag = useCallback(() => {
    if (rafRef.current) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = 0;
    }
    if (dragPreviewElRef.current) {
      dragPreviewElRef.current.remove();
      dragPreviewElRef.current = null;
    }
    dragSessionRef.current = null;
    dragPendingRef.current = false;
    isDraggingRef.current = false;
    setIsDragging(false);
    setDragOverPath(null);
    lastHoveredFolderRef.current = null;
    endDrag();
  }, [endDrag]);

  /** Hand over to native OS drag-and-drop (Electron startDrag). */
  const triggerNativeDrag = useCallback(() => {
    const session = dragSessionRef.current;
    if (!session || !window.electron) return;
    window.electron.startDrag(session.files.map((f) => f.path));
  }, []);

  // ── mousemove (document) ────────────────────────────────────
  const handleDocumentMouseMove = useCallback(
    (e: MouseEvent) => {
      if (!dragPendingRef.current && !isDraggingRef.current) return;

      if (dragPendingRef.current) {
        const session = dragSessionRef.current;
        if (!session) return;
        const dx = e.clientX - session.startX;
        const dy = e.clientY - session.startY;
        if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) return;

        // Enter drag mode
        dragPendingRef.current = false;
        isDraggingRef.current = true;
        setIsDragging(true);
        startDrag(session.files, session.sourcePath);
        createDragPreview(session.files, e.clientX, e.clientY);
        return;
      }

      // Update drag preview position (throttled via rAF)
      if (!rafRef.current) {
        rafRef.current = requestAnimationFrame(() => {
          rafRef.current = 0;
          if (dragPreviewElRef.current) {
            dragPreviewElRef.current.style.left = `${e.clientX + DRAG_PREVIEW_OFFSET_X}px`;
            dragPreviewElRef.current.style.top = `${e.clientY + DRAG_PREVIEW_OFFSET_Y}px`;
          }

          // Check hover target for internal folder drops
          const hoveredFile = findFolderAtPoint(e.clientX, e.clientY);
          const prev = lastHoveredFolderRef.current;
          if (hoveredFile?.path !== prev?.path) {
            lastHoveredFolderRef.current = hoveredFile;
            setDragOverPath(hoveredFile?.path ?? null);
          }
        });
      }
    },
    [isDragging, startDrag, createDragPreview, findFolderAtPoint],
  );

  // ── mouseup (document) ──────────────────────────────────────
  const handleDocumentMouseUp = useCallback(
    (e: MouseEvent) => {
      if (!isDraggingRef.current) {
        dragPendingRef.current = false;
        dragSessionRef.current = null;
        return;
      }

      const hoveredFile = findFolderAtPoint(e.clientX, e.clientY);
      const session = dragSessionRef.current;

      if (hoveredFile && session && onDropOnFolder) {
        const operation: "move" | "copy" = e.shiftKey ? "copy" : "move";
        onDropOnFolder(session.files, hoveredFile.path, operation);
      }

      cleanupDrag();
    },
    [isDragging, findFolderAtPoint, onDropOnFolder, cleanupDrag],
  );

  // ── mouseout (window) → hand over to native drag ────────────
  const handleWindowMouseOut = useCallback(
    (e: MouseEvent) => {
      if (!isDraggingRef.current) return;
      // Only fire when truly leaving the window (not entering a child element)
      if (e.relatedTarget !== null) return;
      // User dragged outside the window — switch to OS-level drag
      triggerNativeDrag();
      cleanupDrag();
    },
    [isDragging, triggerNativeDrag, cleanupDrag],
  );

  // ── keydown (Escape) ────────────────────────────────────────
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!isDraggingRef.current && !dragPendingRef.current) return;
      if (e.key === "Escape") {
        cleanupDrag();
      }
    },
    [isDragging, cleanupDrag],
  );

  // Register document/window listeners during drag
  useEffect(() => {
    if (!isDragging) return;

    document.addEventListener("mousemove", handleDocumentMouseMove);
    document.addEventListener("mouseup", handleDocumentMouseUp);
    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener("mouseout", handleWindowMouseOut);

    return () => {
      document.removeEventListener("mousemove", handleDocumentMouseMove);
      document.removeEventListener("mouseup", handleDocumentMouseUp);
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("mouseout", handleWindowMouseOut);
    };
  }, [isDragging, handleDocumentMouseMove, handleDocumentMouseUp, handleKeyDown, handleWindowMouseOut]);

  // ═══════════════════════════════════════════════════════════════
  //  End of drag simulation
  // ═══════════════════════════════════════════════════════════════

  /** Mouse-down on a file row — prepares for potential drag. */
  const handleItemMouseDown = useCallback(
    (e: React.MouseEvent, file: IFile) => {
      if (e.button !== 0) return;
      if (renamingPath) return;
      if (didSelectRef.current) {
        didSelectRef.current = false;
        return;
      }

      const filesToDrag = selectedFiles.has(file.path)
        ? files.filter((f) => selectedFiles.has(f.path))
        : [file];

      dragSessionRef.current = {
        files: filesToDrag,
        sourcePath: currentPath || "",
        startX: e.clientX,
        startY: e.clientY,
      };
      dragPendingRef.current = true;
    },
    [selectedFiles, files, currentPath, renamingPath],
  );

  // --- Rubber-band selection ---

  const rowHeight = useCallback((_index: number, rowProps: RowData) => {
    const item = rowProps.items[_index];
    if (!item) return 0;
    if (item.kind === "header") return HEADER_HEIGHT;
    if (item.kind === "file") return LIST_ROW_HEIGHT(rowProps.iconSize);
    return GRID_ROW_HEIGHT(rowProps.iconSize);
  }, []);

  return (
    <div
      ref={containerRef}
      className={`file-list-container${isDragging ? " is-dragging" : ""}`}
      style={{ width: "100%", height: "100%", position: "relative" }}
      onMouseDown={handleBackgroundMouseDown}
      onContextMenu={(e) => {
        if ((e.target as HTMLElement).closest?.(".file-rename-input")) return;
        e.preventDefault();
        if (
          !(e.target as HTMLElement).closest(
            ".file-list-item, .file-group-header",
          )
        ) {
          onBackgroundContextMenu?.(e);
        }
      }}
      onClick={(e) => {
        if (
          !(e.target as HTMLElement).closest(
            ".file-list-item, .file-group-header",
          )
        ) {
          if (didSelectRef.current) {
            didSelectRef.current = false;
            return;
          }
          lastClickRef.current = null;
          if (renameTimeoutRef.current !== null) {
            clearTimeout(renameTimeoutRef.current);
            renameTimeoutRef.current = null;
          }
          if (renamingPath) setRenamingPath(null);
          onDeselectAll?.();
          document.activeElement?.blur();
        }
      }}
    >
      <AutoSizer
        renderProp={({ height, width }) => {
          if (height == null || width == null) return null;
          const columns =
            viewMode === "grid"
              ? Math.max(1, Math.floor((width + 8) / (iconSize + 40)))
              : 0;

          const items = flattenItems(files, groupingEnabled, viewMode, columns);

          itemBoxesRef.current = computeItemBoxes(
            items,
            columns,
            width,
            iconSize,
          );

          const rowPropsData: RowData = {
            items,
            selectedFiles,
            failedImages,
            renamingPath,
            renameValue,
            onSelect,
            onNavigate,
            onRename,
            onContextMenu,
            onImageError: handleImageError,
            onItemClick: handleItemClick,
            onItemDoubleClick: handleItemDoubleClick,
            onItemMouseDown: handleItemMouseDown,
            onRenameInputChange: handleRenameInputChange,
            onRenameSubmit: handleRenameSubmit,
            onRenameCancel: handleRenameCancel,
            onHoverFile,
            dragOverPath,
            iconSize,
            filledIcons,
            viewMode,
            columns,
            marqueeEnabled,
          };

          return (
            <List
              listRef={listImperativeRef}
              style={{ height, width, maxHeight: height }}
              rowComponent={Row}
              rowProps={rowPropsData}
              rowCount={items.length}
              rowHeight={rowHeight}
              overscanCount={5}
            />
          );
        }}
      />
      {selectionBox && selectionBox.w > 0 && (
        <div
          className="selection-box"
          style={{
            position: "absolute",
            left: selectionBox.x,
            top: selectionBox.y,
            width: selectionBox.w,
            height: selectionBox.h,
            pointerEvents: "none",
            zIndex: 9999,
          }}
        />
      )}
      {/* Transparent overlay — blocks hover/click effects on underlying
          elements during drag while allowing elementFromPoint to see through */}
      {isDragging && (
        <div
          className="drag-overlay"
          style={{
            position: "absolute",
            inset: 0,
            zIndex: 9998,
            pointerEvents: "all",
          }}
          onMouseEnter={(e) => {
            // Prevent propagation of mouseenter to underlying elements
            e.stopPropagation();
          }}
        />
      )}
    </div>
  );
};

export const FileList = memo(FileListComponent);
