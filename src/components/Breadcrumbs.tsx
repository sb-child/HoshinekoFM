import React, { useRef, useEffect, useState, useCallback } from "react";
import "./Breadcrumbs.css";
import { Button } from "./Button";
import { IconButton } from "./IconButton";
import { Icon } from "./Icon";
import { Chip } from "./md";
import { ContextMenu } from "./ContextMenu";
import { useDrag } from "../contexts/DragContext";
import { clearPendingNativeDrag } from "./FileList";
import type { IFile } from "../types/files";
import { t } from "../i18n";

type HomeMap = Record<string, { username: string; uid: number }>;

/**
 * 从 homeMap 中找到最深匹配的用户家目录。
 * 按路径长度降序排列键，返回首个前缀匹配的 home（路径最长的匹配优先）。
 *
 * @param currentPath - 当前路径（已 sanitize 开头为 `/`）
 * @param map - /etc/passwd 解析出的 home → { username, uid } 映射
 * @returns 匹配的 `{ path, username }`，无匹配时返回 `null`
 */
function findBestHome(currentPath: string, map: HomeMap): { path: string; username: string } | null {
  const keys = Object.keys(map).sort((a, b) => b.length - a.length);
  for (const home of keys) {
    if (currentPath === home || currentPath.startsWith(home + "/")) {
      return { path: home, username: map[home].username };
    }
  }
  return null;
}

interface BreadcrumbsProps {
  currentPath: string;
  onNavigate: (path: string) => void;
  onDropFiles: (
    targetPath: string,
    files: IFile[],
    operation: "move" | "copy",
  ) => void;
  onDropExternalFiles: (targetPath: string, filePaths: string[]) => void;
}

interface SymlinkInfo {
  isSymlink: boolean;
  target?: string;
}

interface BreadcrumbCtxMenuState {
  x: number;
  y: number;
  realPath: string;
  isChip: boolean;
}

export const Breadcrumbs: React.FC<BreadcrumbsProps> = ({
  currentPath,
  onNavigate,
  onDropFiles,
  onDropExternalFiles,
}) => {
  const sanitizedPath = currentPath.startsWith("/")
    ? currentPath
    : "/" + currentPath;
  const parts = sanitizedPath.split("/").filter(Boolean);
  const scrollRef = useRef<HTMLDivElement>(null);
  const lastRef = useRef<HTMLSpanElement>(null);

  const [homeMap, setHomeMap] = useState<HomeMap>({});
  const [dragOverPath, setDragOverPath] = useState<string | null>(null);
  const [symlinkInfo, setSymlinkInfo] = useState<Map<string, SymlinkInfo>>(
    new Map(),
  );
  const [breadcrumbCtxMenu, setBreadcrumbCtxMenu] =
    useState<BreadcrumbCtxMenuState | null>(null);
  const { getDragState, endDrag } = useDrag();

  useEffect(() => {
    window.electron
      .getHomeMap()
      .then(setHomeMap)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (lastRef.current) {
      lastRef.current.scrollIntoView({ block: "nearest", inline: "end" });
    }
  }, [currentPath]);

  useEffect(() => {
    const segmentPaths = parts.map(
      (_, i) => "/" + parts.slice(0, i + 1).join("/"),
    );
    if (segmentPaths.length === 0) return;

    let cancelled = false;
    const check = async () => {
      try {
        if (window.electron.checkSymlinks) {
          const results = await window.electron.checkSymlinks(segmentPaths);
          if (cancelled) return;
          const info = new Map<string, SymlinkInfo>();
          for (const r of results) {
            info.set(r.path, { isSymlink: r.isSymlink, target: r.target });
          }
          setSymlinkInfo(info);
        }
      } catch {
        // ignore errors
      }
    };
    check();
    return () => {
      cancelled = true;
    };
  }, [currentPath, parts]);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = e.shiftKey ? "copy" : "move";
  }, []);

  const handleDragEnter = useCallback(
    (e: React.DragEvent, targetPath: string) => {
      e.preventDefault();
      e.stopPropagation();
      setDragOverPath(targetPath);
    },
    [],
  );

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    const el = e.currentTarget as HTMLElement;
    if (e.relatedTarget && el.contains(e.relatedTarget as Node)) return;
    e.preventDefault();
    e.stopPropagation();
    setDragOverPath(null);
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent, targetPath: string) => {
      e.preventDefault();
      e.stopPropagation();
      setDragOverPath(null);

      const dragState = getDragState();
      if (dragState && dragState.files.length > 0) {
        if (dragState.sourcePath === targetPath) {
          return;
        }
        const operation: "move" | "copy" = e.shiftKey ? "copy" : "move";
        clearPendingNativeDrag();
        onDropFiles(targetPath, dragState.files, operation);
        endDrag();
        return;
      }

      const externalPaths = Array.from(e.dataTransfer.files)
        .filter((f) => (f as unknown as { path?: string }).path)
        .map((f) => (f as unknown as { path: string }).path);
      if (externalPaths.length > 0) {
        onDropExternalFiles(targetPath, externalPaths);
      }
    },
    [getDragState, onDropFiles, onDropExternalFiles, endDrag],
  );

  const handleBreadcrumbContextMenu = useCallback(
    (e: React.MouseEvent, realPath: string, isChip = false) => {
      e.preventDefault();
      e.stopPropagation();
      setBreadcrumbCtxMenu({ x: e.clientX, y: e.clientY, realPath, isChip });
    },
    [],
  );

  const matchedHome = findBestHome(sanitizedPath, homeMap);
  const isInHome = matchedHome !== null;
  const homeMatchPath = matchedHome?.path ?? null;
  const homeSegments = matchedHome ? matchedHome.path.split("/").filter(Boolean) : [];
  const remainingParts = isInHome ? parts.slice(homeSegments.length) : parts;
  const ownerName = matchedHome?.username ?? "";

  const displayParts = isInHome ? remainingParts : parts;
  const homeSegmentCount = isInHome ? homeSegments.length : 0;

  return (
    <div
      ref={scrollRef}
      onWheel={(e) => {
        if (scrollRef.current) {
          scrollRef.current.scrollLeft += e.deltaY;
        }
      }}
      style={{
        display: "flex",
        alignItems: "center",
        overflowX: "auto",
        overflowY: "hidden",
        whiteSpace: "nowrap",
        scrollbarWidth: "none",
        WebkitOverflowScrolling: "touch",
      }}
    >
      {isInHome && homeMatchPath ? (
        <Chip
          title={t("breadcrumbs.home", ownerName, homeMatchPath)}
          onClick={() => onNavigate(homeMatchPath)}
          onDragOver={handleDragOver}
          onDragEnter={(e) => handleDragEnter(e, homeMatchPath)}
          onDragLeave={handleDragLeave}
          onDrop={(e) => handleDrop(e, homeMatchPath)}
          onContextMenu={(e) => handleBreadcrumbContextMenu(e, homeMatchPath, true)}
          className={`breadcrumb-chip${dragOverPath === homeMatchPath ? " drag-over" : ""}`}
        >
          <Icon name="home" slot="icon" />
          {ownerName}
        </Chip>
      ) : (
        <>
          <IconButton
            variant="standard"
            onClick={() => onNavigate("/")}
            onDragOver={handleDragOver}
            onDragEnter={(e) => handleDragEnter(e, "/")}
            onDragLeave={handleDragLeave}
            onDrop={(e) => handleDrop(e, "/")}
            className={`breadcrumb-root${dragOverPath === "/" ? " drag-over" : ""}`}
            title={t("breadcrumbs.root")}
          >
            <Icon name="home" />
          </IconButton>
          {parts.length === 0 && (
            <span style={{ fontWeight: 600, padding: "0 2px" }}>/</span>
          )}
        </>
      )}

      {displayParts.map((p, i) => {
        const absoluteIndex = homeSegmentCount + i;
        const segmentPath = "/" + parts.slice(0, absoluteIndex + 1).join("/");
        const info = symlinkInfo.get(segmentPath);
        const isSymlinkDir = info?.isSymlink ?? false;
        const symlinkTarget = info?.target;
        const isLast = i === displayParts.length - 1;

        return (
          <React.Fragment key={segmentPath}>
            <span
              ref={isLast ? lastRef : undefined}
              className={`breadcrumb-separator${dragOverPath === segmentPath ? " drag-over" : ""}`}
            >
              /
            </span>
            <Button
              variant="text"
              onClick={() => {
                onNavigate(segmentPath);
              }}
              onDragOver={handleDragOver}
              onDragEnter={(e) => handleDragEnter(e, segmentPath)}
              onDragLeave={handleDragLeave}
              onDrop={(e) => handleDrop(e, segmentPath)}
              onContextMenu={(e) => {
                if (isSymlinkDir && symlinkTarget) {
                  handleBreadcrumbContextMenu(e, symlinkTarget);
                }
              }}
              className={`breadcrumb-item${isSymlinkDir ? " symlink" : ""}${dragOverPath === segmentPath ? " drag-over" : ""}`}
              style={{ fontWeight: isLast ? 600 : 400 }}
              title={
                isSymlinkDir && symlinkTarget
                  ? t("symlink.tooltip", symlinkTarget)
                  : undefined
              }
            >
              {p}
            </Button>
          </React.Fragment>
        );
      })}

      {breadcrumbCtxMenu && (
        <ContextMenu
          x={breadcrumbCtxMenu.x}
          y={breadcrumbCtxMenu.y}
          items={
            breadcrumbCtxMenu.isChip
              ? [
                {
                  label: t("breadcrumbs.go_to_root"),
                  icon: "home",
                  action: () => {
                    onNavigate("/");
                    setBreadcrumbCtxMenu(null);
                  },
                },
              ]
              : [
                {
                  label: t("symlink.go_to_target"),
                  icon: "arrow_forward",
                  action: () => {
                    onNavigate(breadcrumbCtxMenu.realPath);
                    setBreadcrumbCtxMenu(null);
                  },
                },
              ]
          }
          onClose={() => setBreadcrumbCtxMenu(null)}
        />
      )}
    </div>
  );
};
