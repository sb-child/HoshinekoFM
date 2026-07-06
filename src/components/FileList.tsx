import { useState, useCallback, useRef, useEffect, memo } from "react";
import type { IFile } from "../types/files";
import "./FileList.css";
import { AutoSizer } from "react-virtualized-auto-sizer";
import { List, useListRef } from "react-window";
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

const FileListComponent: React.FC<FileListProps> = ({
  files,
  selectedFiles,
  onSelect,
  onNavigate,
  onRename,
  onContextMenu,
  onBackgroundContextMenu,
  onDeselectAll,
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

  const lastClickRef = useRef<{ path: string; time: number } | null>(null);
  const lastDragRef = useRef<{ path: string; time: number } | null>(null);
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

    // Compute flattened index using the same logic as the render
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

  // --- Item click ---
  const handleItemClick = useCallback(
    (e: React.MouseEvent, file: IFile) => {
      (document.activeElement as HTMLElement)?.blur?.();
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

      if (
        lastDragRef.current?.path === file.path &&
        now - lastDragRef.current.time < 100
      ) {
        lastDragRef.current = null;
        return;
      }
      lastDragRef.current = null;
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
      className="file-list-container"
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
          lastDragRef.current = null;
          if (renameTimeoutRef.current !== null) {
            clearTimeout(renameTimeoutRef.current);
            renameTimeoutRef.current = null;
          }
          if (renamingPath) setRenamingPath(null);
          onDeselectAll?.();
          (document.activeElement as HTMLElement)?.blur?.();
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
            onRenameInputChange: handleRenameInputChange,
            onRenameSubmit: handleRenameSubmit,
            onRenameCancel: handleRenameCancel,
            onHoverFile,
            iconSize,
            filledIcons,
            viewMode,
            columns,
            marqueeEnabled,
            allFiles: files,
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
    </div>
  );
};

export const FileList = memo(FileListComponent);
