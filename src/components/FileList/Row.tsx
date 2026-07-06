import { useCallback, useEffect, useRef } from "react";
import type { RowComponentProps } from "react-window";
import type { IFile } from "../../types/files";
import { Icon } from "../Icon";
import { MarqueeText } from "../MarqueeText";
import {
  type ListItem,
  getFileTitle,
  getFileIconFromMime,
  formatSize,
  listSpacing,
} from "./utils";
import { useFileDraggable, useFolderDroppable, useDragOver } from "../../utils/dnd";

interface RowData {
  items: ListItem[];
  selectedFiles: Set<string>;
  failedImages: Set<string>;
  renamingPath: string | null;
  renameValue: string;
  onSelect: (file: IFile, toggle: boolean, range: boolean) => void;
  onNavigate: (file: IFile) => void;
  onRename?: (file: IFile, newName: string) => void;
  onContextMenu?: (e: React.MouseEvent, file: IFile) => void;
  onImageError: (path: string) => void;
  onItemClick: (e: React.MouseEvent, file: IFile) => void;
  onItemDoubleClick: (file: IFile) => void;
  onRenameInputChange: (value: string) => void;
  onRenameSubmit: () => void;
  onRenameCancel: () => void;
  onHoverFile?: (file: IFile | null) => void;
  iconSize: number;
  filledIcons: boolean;
  viewMode: "grid" | "list";
  columns: number;
  marqueeEnabled: boolean;
  /** 当前目录所有文件（用于 dnd-kit 拖拽） */
  allFiles: IFile[];
}

function FileIconDisplay({
  file,
  iconSize,
  filledIcons,
  hasFailed,
}: {
  file: IFile;
  iconSize: number;
  filledIcons: boolean;
  hasFailed: boolean;
}) {
  const isImg = file.mime?.startsWith("image/") ?? false;
  const isBrokenSymlink = file.symlinkTarget
    ? file.mime === "inode/symlink"
    : false;

  return (
    <span
      className="file-icon"
      style={{
        width: `${iconSize}px`,
        height: `${iconSize}px`,
        fontSize: `${iconSize}px`,
      }}
    >
      {isImg && !hasFailed && (
        <img
          src={`media://${file.path}`}
          alt={file.name}
          className="file-thumbnail"
          draggable={false}
          loading="lazy"
          decoding="async"
          style={{
            width: `${iconSize}px`,
            height: `${iconSize}px`,
            objectFit: "cover",
          }}
        />
      )}
      {(!isImg || hasFailed) && (
        <span className="file-icon-stack">
          <Icon
            name={
              isBrokenSymlink
                ? "link_off"
                : getFileIconFromMime(file.mime, file.isDirectory)
            }
            filled={filledIcons}
            className={
              file.isDirectory
                ? "folder-icon"
                : isBrokenSymlink
                  ? "doc-icon broken-symlink-icon"
                  : "doc-icon"
            }
            style={{
              fontSize: `${iconSize}px`,
              ...(isBrokenSymlink ? { color: "#ef5350" } : {}),
            }}
          />
          {file.isMountpoint &&
            file.isDirectory &&
            file.mountSource?.startsWith("/dev/") && (
            <Icon
              name="hard_drive"
              className="mountpoint-badge"
              style={{
                position: "absolute",
                bottom: "-1px",
                right: "-2px",
                fontSize: `${Math.max(10, iconSize * 0.45)}px`,
                color: "var(--md-sys-color-primary)",
              }}
            />
          )}
        </span>
      )}
    </span>
  );
}

function FileNameDisplay({
  file,
  isRenaming,
  renameValue,
  renameInputRef,
  onRenameInputChange,
  onRenameSubmit,
  onRenameCancel,
  style,
  marqueeTextStyle,
  marqueeEnabled,
}: {
  file: IFile;
  isRenaming: boolean;
  renameValue: string;
  renameInputRef: (el: HTMLInputElement | null) => void;
  onRenameInputChange: (value: string) => void;
  onRenameSubmit: () => void;
  onRenameCancel: () => void;
  style?: React.CSSProperties;
  marqueeTextStyle?: React.CSSProperties;
  marqueeEnabled: boolean;
}) {
  const isSymlink = !!file.symlinkTarget;

  if (isRenaming) {
    return (
      <input
        ref={renameInputRef}
        className="file-rename-input"
        type="text"
        value={renameValue}
        autoFocus
        onChange={(e) => onRenameInputChange(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") {
            onRenameSubmit();
          } else if (e.key === "Escape") {
            onRenameCancel();
          }
        }}
        onBlur={() => onRenameSubmit()}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        style={style}
      />
    );
  }

  return (
    <span className={`file-name${isSymlink ? " symlink" : ""}`} style={style}>
      <MarqueeText
        enabled={marqueeEnabled}
        className="file-name-text"
        style={marqueeTextStyle}
        title={getFileTitle(file)}
      >
        {file.name}
      </MarqueeText>
    </span>
  );
}

/**
 * 可拖拽的文件项（使用 dnd-kit）。
 *
 * 文件项使用 useDraggable，文件夹额外注册 useDroppable（供 collisionDetection 测量）。
 * 拖拽高亮由 DragOverContext 管理（不依赖 useDroppable 的 isOver，避免与 useDraggable 冲突）。
 * 通过 data-droppable-id 属性供 Tauri onDragDropEvent 查找目标文件夹。
 */
function DraggableFileItem({
  data,
  file,
  sp,
  triggerRipple,
  renameInputRef,
}: {
  data: RowData;
  file: IFile;
  sp: ReturnType<typeof listSpacing>;
  triggerRipple: (e: React.MouseEvent, el: HTMLElement | null) => void;
  renameInputRef: (el: HTMLInputElement | null) => void;
}) {
  const isSelected = data.selectedFiles.has(file.path);
  const hasFailed = data.failedImages.has(file.path);
  const isRenaming = data.renamingPath === file.path;

  // dnd-kit: 文件可拖拽
  const {
    attributes: dragAttributes,
    listeners: dragListeners,
    setNodeRef: setDragRef,
    isDragging,
  } = useFileDraggable(file, data.selectedFiles, data.allFiles);

  // dnd-kit: 文件夹可放置（注册 droppable DOM 节点供 collisionDetection 测量）
  const { setNodeRef: setDropRef } = useFolderDroppable(file);

  // 从 DragOverContext 读取当前悬停目标（替代 useDroppable 的 isOver）
  const dragOverPath = useDragOver();
  const isOver = file.isDirectory && dragOverPath === file.path;

  // 合并 drag 和 drop 的 ref
  const setRef = useCallback(
    (el: HTMLDivElement | null) => {
      setDragRef(el);
      setDropRef(el);
    },
    [setDragRef, setDropRef],
  );

  return (
    <div
      ref={setRef}
      className={`file-list-item ${isSelected ? "selected" : ""} ${isOver ? "drag-over" : ""}`}
      style={{
        display: "flex",
        alignItems: "center",
        gap: `${sp.gap}px`,
        padding: `${sp.paddingV}px ${sp.paddingH}px`,
        margin: `${sp.marginV}px ${sp.marginH}px`,
        height: `calc(100% - ${sp.marginV * 2}px)`,
        borderRadius: `${sp.borderRadius}px`,
        cursor: "pointer",
        boxSizing: "border-box",
        opacity: isDragging ? 0.5 : 1,
      }}
      data-droppable-id={file.isDirectory ? `folder:${file.path}` : undefined}
      onMouseEnter={() => data.onHoverFile?.(file)}
      onMouseLeave={() => data.onHoverFile?.(null)}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        triggerRipple(e, e.currentTarget as HTMLElement);
      }}
      onClick={(e) => data.onItemClick(e, file)}
      onDoubleClick={() => data.onItemDoubleClick(file)}
      onContextMenu={(e) => {
        if ((e.target as HTMLElement).closest?.(".file-rename-input")) return;
        e.preventDefault();
        e.stopPropagation();
        data.onContextMenu?.(e, file);
      }}
      {...dragAttributes}
      {...dragListeners}
      tabIndex={0}
      role="button"
    >
      <FileIconDisplay
        file={file}
        iconSize={data.iconSize}
        filledIcons={data.filledIcons}
        hasFailed={hasFailed}
      />
      <FileNameDisplay
        file={file}
        isRenaming={isRenaming}
        renameValue={data.renameValue}
        renameInputRef={renameInputRef}
        onRenameInputChange={data.onRenameInputChange}
        onRenameSubmit={data.onRenameSubmit}
        onRenameCancel={data.onRenameCancel}
        style={{ flex: 1, minWidth: 0 }}
        marqueeTextStyle={{ paddingRight: sp.paddingH }}
        marqueeEnabled={data.marqueeEnabled}
      />
      {!isRenaming && (
        <span
          className="file-size"
          style={{ flexShrink: 0, width: "100px", textAlign: "right" }}
        >
          {file.isDirectory ? "" : formatSize(file.size)}
        </span>
      )}
    </div>
  );
}

function GridRowItem({
  data,
  file,
  triggerRipple,
  renameInputRef,
}: {
  data: RowData;
  file: IFile;
  triggerRipple: (e: React.MouseEvent, el: HTMLElement | null) => void;
  renameInputRef: (el: HTMLInputElement | null) => void;
}) {
  const isSelected = data.selectedFiles.has(file.path);
  const hasFailed = data.failedImages.has(file.path);
  const isRenaming = data.renamingPath === file.path;

  // dnd-kit: 文件可拖拽
  const {
    attributes: dragAttributes,
    listeners: dragListeners,
    setNodeRef: setDragRef,
    isDragging,
  } = useFileDraggable(file, data.selectedFiles, data.allFiles);

  // dnd-kit: 文件夹可放置（注册 droppable DOM 节点供 collisionDetection 测量）
  const { setNodeRef: setDropRef } = useFolderDroppable(file);

  // 从 DragOverContext 读取当前悬停目标（替代 useDroppable 的 isOver）
  const dragOverPath = useDragOver();
  const isOver = file.isDirectory && dragOverPath === file.path;

  // 合并 drag 和 drop 的 ref
  const setRef = useCallback(
    (el: HTMLDivElement | null) => {
      setDragRef(el);
      setDropRef(el);
    },
    [setDragRef, setDropRef],
  );

  return (
    <div
      ref={setRef}
      key={file.path}
      className={`file-list-item file-grid-item ${isSelected ? "selected" : ""} ${isOver ? "drag-over" : ""}`}
      style={{ opacity: isDragging ? 0.5 : 1 }}
      data-droppable-id={file.isDirectory ? `folder:${file.path}` : undefined}
      onMouseEnter={() => data.onHoverFile?.(file)}
      onMouseLeave={() => data.onHoverFile?.(null)}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        triggerRipple(e, e.currentTarget as HTMLElement);
      }}
      onClick={(e) => data.onItemClick(e, file)}
      onDoubleClick={() => data.onItemDoubleClick(file)}
      onContextMenu={(e) => {
        if ((e.target as HTMLElement).closest?.(".file-rename-input")) return;
        e.preventDefault();
        e.stopPropagation();
        data.onContextMenu?.(e, file);
      }}
      {...dragAttributes}
      {...dragListeners}
      tabIndex={0}
      role="button"
    >
      <FileIconDisplay
        file={file}
        iconSize={data.iconSize}
        filledIcons={data.filledIcons}
        hasFailed={hasFailed}
      />
      <FileNameDisplay
        file={file}
        isRenaming={isRenaming}
        renameValue={data.renameValue}
        renameInputRef={renameInputRef}
        onRenameInputChange={data.onRenameInputChange}
        onRenameSubmit={data.onRenameSubmit}
        onRenameCancel={data.onRenameCancel}
        style={
          isRenaming
            ? {
              textAlign: "center",
              fontSize: "12px",
              marginTop: "2px",
              width: "100%",
              maxWidth: "100%",
              boxSizing: "border-box",
            }
            : {
              textAlign: "center",
              fontSize: "12px",
              maxWidth: "100%",
              width: "100%",
              marginTop: "2px",
              display: "block",
            }
        }
        marqueeTextStyle={{ paddingLeft: 0, paddingRight: 0 }}
        marqueeEnabled={data.marqueeEnabled}
      />
    </div>
  );
}

function Row({ index, style, ...data }: RowComponentProps<RowData>) {
  const item = data.items[index];

  const renameInputCleanupRef = useRef<(() => void) | null>(null);

  const renameInputRef = useCallback((el: HTMLInputElement | null) => {
    renameInputCleanupRef.current?.();
    renameInputCleanupRef.current = null;
    if (!el) return;
    const handler = (e: DragEvent) => {
      e.stopImmediatePropagation();
    };
    el.addEventListener("dragstart", handler, true);
    renameInputCleanupRef.current = () => {
      el.removeEventListener("dragstart", handler, true);
    };
  }, []);

  useEffect(() => {
    return () => {
      renameInputCleanupRef.current?.();
    };
  }, []);

  const triggerRipple = useCallback(
    (e: React.MouseEvent, el: HTMLElement | null) => {
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const size = Math.max(rect.width, rect.height) * 2;
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;

      const ripple = document.createElement("span");
      ripple.className = "file-ripple";
      ripple.style.width = `${size}px`;
      ripple.style.height = `${size}px`;
      ripple.style.left = `${x}px`;
      ripple.style.top = `${y}px`;
      el.appendChild(ripple);

      const cleanup = () => {
        ripple.removeEventListener("animationend", cleanup);
        ripple.remove();
      };
      ripple.addEventListener("animationend", cleanup);
    },
    [],
  );

  if (item.kind === "header") {
    return (
      <div
        style={{
          ...style,
          padding: "20px 2px 8px",
          fontWeight: 500,
          color: "var(--md-sys-color-primary)",
          borderBottom: "1px solid var(--md-sys-color-outline-variant)",
          boxSizing: "border-box",
        }}
      >
        {item.label}
      </div>
    );
  }

  if (item.kind === "file") {
    const sp = listSpacing(data.iconSize);
    return (
      <div style={style}>
        <DraggableFileItem
          data={data}
          file={item.file}
          sp={sp}
          triggerRipple={triggerRipple}
          renameInputRef={renameInputRef}
        />
      </div>
    );
  }

  const { files } = item;
  return (
    <div
      className="grid-row-container"
      style={{
        ...style,
        gridTemplateColumns: `repeat(${data.columns}, 1fr)`,
      }}
    >
      {files.map((file) => (
        <GridRowItem
          key={file.path}
          data={data}
          file={file}
          triggerRipple={triggerRipple}
          renameInputRef={renameInputRef}
        />
      ))}
    </div>
  );
}

export { Row };
export type { RowData };
