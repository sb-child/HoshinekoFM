import React, { useRef, useEffect, useState, useCallback } from "react";
import "./Breadcrumbs.css";
import { Button } from "./Button";
import { IconButton } from "./IconButton";
import { Icon } from "./Icon";
import { Chip } from "./md";
import { ContextMenu } from "./ContextMenu";
import { t } from "../i18n";
import type { BreadcrumbEntry } from "../types/tauriEvents";

/** 特殊挂载源 → 显示配置 */
interface MountDisplayConfig {
  icon: string;
  labelKey: string;
  titleKey: string;
  showPath: boolean;
}

const MOUNT_SOURCE_DISPLAY: Record<string, MountDisplayConfig> = {
  devtmpfs: { icon: 'devices', labelKey: 'breadcrumbs.dev', titleKey: 'breadcrumbs.dev_title', showPath: false },
  devpts:   { icon: 'terminal_2', labelKey: 'breadcrumbs.devpts', titleKey: 'breadcrumbs.devpts_title', showPath: false },
  proc:     { icon: 'developer_board', labelKey: 'breadcrumbs.proc', titleKey: 'breadcrumbs.proc_title', showPath: false },
  sysfs:    { icon: 'stacks', labelKey: 'breadcrumbs.sysfs', titleKey: 'breadcrumbs.sysfs_title', showPath: false },
  tmpfs:    { icon: 'auto_delete', labelKey: 'breadcrumbs.tmpfs', titleKey: 'breadcrumbs.tmpfs_title', showPath: true },
};

/** 通常唯一的挂载源（全局仅一个挂载点） */
const UNIQUE_SOURCES = new Set(["devtmpfs", "devpts", "proc", "sysfs"]);

function isUniqueSource(source: string | null): boolean {
  return source !== null && UNIQUE_SOURCES.has(source);
}

function chipLabel(
  config: MountDisplayConfig,
  mountpoint: string,
  bold: boolean,
  folded: boolean,
  italic: boolean,
): React.ReactNode {
  const base = t(config.labelKey);
  const lastSeg = mountpoint.split('/').filter(Boolean).pop() ?? mountpoint;

  const content = folded
    ? (bold ? <span style={{ fontWeight: 600 }}>{lastSeg}</span> : lastSeg)
    : config.showPath
      ? (bold ? <>{base} <span style={{ fontWeight: 600 }}>{lastSeg}</span></> : <>{base} {lastSeg}</>)
      : (bold ? <span style={{ fontWeight: 600 }}>{base}</span> : base);

  return italic ? <span style={{ fontStyle: 'italic' }}>{content}</span> : content;
}

interface BreadcrumbsProps {
  entries: BreadcrumbEntry[];
  onNavigate: (path: string) => void;
}

interface CtxMenuState {
  x: number;
  y: number;
  targetPath: string;
  symlinkTarget?: string;
  isChip: boolean;
}

/**
 * 面包屑导航 — 纯 props 组件。
 *
 * 所有数据（home / mount / symlink）由后端 `hf:breadcrumbs` 事件推送，
 * 前端仅负责渲染，不调用任何 Electron IPC。
 */
export const Breadcrumbs: React.FC<BreadcrumbsProps> = ({
  entries,
  onNavigate,
}) => {
  const scrollRef = useRef<HTMLDivElement>(null);
  const lastRef = useRef<HTMLSpanElement>(null);
  const [ctxMenu, setCtxMenu] = useState<CtxMenuState | null>(null);

  useEffect(() => {
    if (lastRef.current) {
      lastRef.current.scrollIntoView({ block: "nearest", inline: "end" });
    }
  }, [entries]);

  const handleContextMenu = useCallback(
    (e: React.MouseEvent, entry: BreadcrumbEntry, isChip: boolean) => {
      e.preventDefault();
      e.stopPropagation();
      setCtxMenu({
        x: e.clientX,
        y: e.clientY,
        targetPath: entry.path,
        symlinkTarget: entry.is_symlink && entry.symlink_target
          ? entry.symlink_target
          : undefined,
        isChip,
      });
    },
    [],
  );

  if (entries.length === 0) {
    return (
      <div ref={scrollRef} className="breadcrumb-container">
        <RootChip onNavigate={onNavigate} onContextMenu={handleContextMenu} alwaysChip />
      </div>
    );
  }

  const homeIdx = entries.findIndex(e => e.is_home);
  const mountIdx = entries.findIndex(
    e => e.is_mount_point && e.mount_source !== null && e.mount_source in MOUNT_SOURCE_DISPLAY,
  );

  const hasHome = homeIdx >= 0;
  const hasMount = !hasHome && mountIdx >= 0;

  return (
    <div
      ref={scrollRef}
      className="breadcrumb-container"
      onWheel={(e) => {
        if (scrollRef.current) {
          scrollRef.current.scrollLeft += e.deltaY;
        }
      }}
    >
      {hasHome ? (
        <HomeMode
          entries={entries}
          homeIdx={homeIdx}
          onNavigate={onNavigate}
          onContextMenu={handleContextMenu}
          lastRef={lastRef}
        />
      ) : hasMount ? (
        <MountMode
          entries={entries}
          mountIdx={mountIdx}
          onNavigate={onNavigate}
          onContextMenu={handleContextMenu}
          lastRef={lastRef}
        />
      ) : (
        <DefaultMode
          entries={entries}
          onNavigate={onNavigate}
          onContextMenu={handleContextMenu}
          lastRef={lastRef}
        />
      )}

      {ctxMenu && (
        <ContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          items={ctxMenu.isChip
            ? [
              ...(ctxMenu.symlinkTarget
                ? [{
                  label: t("symlink.go_to_target"),
                  icon: "arrow_forward",
                  action: () => {
                    onNavigate(ctxMenu.symlinkTarget!);
                    setCtxMenu(null);
                  },
                }]
                : []),
              {
                label: t("breadcrumbs.go_to_root"),
                icon: "home",
                action: () => { onNavigate("/"); setCtxMenu(null); },
              },
            ]
            : [{
              label: t("symlink.go_to_target"),
              icon: "arrow_forward",
              action: () => {
                onNavigate(ctxMenu.targetPath);
                setCtxMenu(null);
              },
            }]
          }
          onClose={() => setCtxMenu(null)}
        />
      )}
    </div>
  );
};

// ─── Root Chip ────────────────────────────────────────────────────────────

const RootChip: React.FC<{
  onNavigate: (path: string) => void;
  onContextMenu: (e: React.MouseEvent, entry: BreadcrumbEntry, isChip: boolean) => void;
  alwaysChip?: boolean;
}> = ({ onNavigate, onContextMenu, alwaysChip }) => {
  const rootEntry: BreadcrumbEntry = {
    name: "",
    path: "/",
    is_symlink: false,
    symlink_target: null,
    is_mount_point: false,
    mount_source: null,
    is_home: false,
    home_username: null,
    accessible: true,
  };

  if (alwaysChip) {
    return (
      <Chip
        title={t("breadcrumbs.root_title", "/")}
        onClick={() => onNavigate("/")}
        onContextMenu={(e) => onContextMenu(e, rootEntry, true)}
        className="breadcrumb-chip"
      >
        <Icon name="tag" slot="icon" />
        <span style={{ fontWeight: 600 }}>{t("breadcrumbs.root")}</span>
      </Chip>
    );
  }

  return (
    <IconButton
      variant="standard"
      onClick={() => onNavigate("/")}
      className="breadcrumb-root"
      title={t("breadcrumbs.root_title", "/")}
    >
      <Icon name="tag" />
    </IconButton>
  );
};

// ─── Home Mode ────────────────────────────────────────────────────────────

const HomeMode: React.FC<{
  entries: BreadcrumbEntry[];
  homeIdx: number;
  onNavigate: (path: string) => void;
  onContextMenu: (e: React.MouseEvent, entry: BreadcrumbEntry, isChip: boolean) => void;
  lastRef: React.RefObject<HTMLSpanElement | null>;
}> = ({ entries, homeIdx, onNavigate, onContextMenu, lastRef }) => {
  const home = entries[homeIdx];
  const suffix = entries.slice(homeIdx + 1);
  const isHomeLast = suffix.length === 0;

  return (
    <>
      <Chip
        title={home.is_symlink && home.symlink_target
          ? `${t("breadcrumbs.home", home.home_username ?? "", home.path)}\n${t("symlink.tooltip", home.symlink_target)}`
          : t("breadcrumbs.home", home.home_username ?? "", home.path)}
        onClick={() => onNavigate(home.path)}
        onContextMenu={(e) => onContextMenu(e, home, true)}
        className="breadcrumb-chip"
      >
        <Icon name="home" slot="icon" />
        {home.is_symlink
          ? <span style={{ fontStyle: 'italic', fontWeight: isHomeLast ? 600 : 400 }}>{home.home_username}</span>
          : isHomeLast
            ? <span style={{ fontWeight: 600 }}>{home.home_username}</span>
            : home.home_username
        }
      </Chip>
      {renderSegments(suffix, onNavigate, onContextMenu, lastRef)}
    </>
  );
};

// ─── Mount Mode ───────────────────────────────────────────────────────────

const MountMode: React.FC<{
  entries: BreadcrumbEntry[];
  mountIdx: number;
  onNavigate: (path: string) => void;
  onContextMenu: (e: React.MouseEvent, entry: BreadcrumbEntry, isChip: boolean) => void;
  lastRef: React.RefObject<HTMLSpanElement | null>;
}> = ({ entries, mountIdx, onNavigate, onContextMenu, lastRef }) => {
  const mount = entries[mountIdx];
  const config = MOUNT_SOURCE_DISPLAY[mount.mount_source!];
  const unique = isUniqueSource(mount.mount_source);
  const prefix = unique ? [] : entries.slice(0, mountIdx);
  const suffix = entries.slice(mountIdx + 1);
  const isMountLast = suffix.length === 0;

  const mountChip = (
    <Chip
      title={mount.is_symlink && mount.symlink_target
        ? `${t(config.titleKey, mount.path)}\n${t("symlink.tooltip", mount.symlink_target)}`
        : t(config.titleKey, mount.path)}
      onClick={() => onNavigate(mount.path)}
      onContextMenu={(e) => onContextMenu(e, mount, true)}
      className="breadcrumb-chip"
    >
      <Icon name={config.icon} slot="icon" />
      {chipLabel(config, mount.path, isMountLast, false, !!(mount.is_symlink && mount.symlink_target))}
    </Chip>
  );

  return (
    <>
      {!unique && <RootChip onNavigate={onNavigate} onContextMenu={onContextMenu} />}
      {renderSegments(prefix, onNavigate, onContextMenu)}
      {prefix.length > 0 && <span className="breadcrumb-separator">/</span>}
      {mountChip}
      {renderSegments(suffix, onNavigate, onContextMenu, lastRef)}
    </>
  );
};

// ─── Default Mode ─────────────────────────────────────────────────────────

const DefaultMode: React.FC<{
  entries: BreadcrumbEntry[];
  onNavigate: (path: string) => void;
  onContextMenu: (e: React.MouseEvent, entry: BreadcrumbEntry, isChip: boolean) => void;
  lastRef: React.RefObject<HTMLSpanElement | null>;
}> = ({ entries, onNavigate, onContextMenu, lastRef }) => {
  return (
    <>
      <RootChip onNavigate={onNavigate} onContextMenu={onContextMenu} />
      {renderSegments(entries, onNavigate, onContextMenu, lastRef)}
    </>
  );
};

// ─── Common Segments ──────────────────────────────────────────────────────

function renderSegments(
  segs: BreadcrumbEntry[],
  onNavigate: (path: string) => void,
  onContextMenu: (e: React.MouseEvent, entry: BreadcrumbEntry, isChip: boolean) => void,
  lastRef?: React.RefObject<HTMLSpanElement | null>,
): React.ReactNode {
  return segs.map((entry, i) => {
    const isLast = !!lastRef && i === segs.length - 1;
    const symlink = entry.is_symlink && entry.symlink_target;

    return (
      <React.Fragment key={entry.path}>
        <span
          ref={isLast ? lastRef : undefined}
          className="breadcrumb-separator"
        >
          /
        </span>
        <Button
          variant="text"
          onClick={() => onNavigate(entry.path)}
          onContextMenu={(e) => {
            if (symlink) onContextMenu(e, entry, false);
          }}
          className={`breadcrumb-item${entry.is_symlink ? " symlink" : ""}`}
          style={{ fontWeight: isLast ? 600 : 400 }}
          title={symlink ? t("symlink.tooltip", entry.symlink_target!) : undefined}
        >
          {entry.name}
        </Button>
      </React.Fragment>
    );
  });
}
