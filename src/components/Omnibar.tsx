import React, { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Breadcrumbs } from "./Breadcrumbs";
import { Icon } from "./Icon";
import { IconButton } from "./IconButton";
import { ContextMenu } from "./ContextMenu";
import type { ContextMenuItem } from "./ContextMenu";
import { t } from "../i18n";
import type { BreadcrumbEntry } from "../types/tauriEvents";
import "./Omnibar.css";

interface OmnibarProps {
  currentPath: string;
  breadcrumbs: BreadcrumbEntry[];
  onNavigate: (path: string) => void;
  onSearch: (query: string, options?: { type?: 'f' | 'd'; minSize?: string; maxSize?: string }) => void;
}

interface OmnibarCtxMenuState {
  x: number;
  y: number;
}

export const Omnibar: React.FC<OmnibarProps> = ({
  currentPath,
  breadcrumbs,
  onNavigate,
  onSearch,
}) => {
  const [isEditing, setIsEditing] = useState(false);
  const [inputValue, setInputValue] = useState(currentPath);
  const inputRef = useRef<HTMLInputElement>(null);
  const [omnibarCtxMenu, setOmnibarCtxMenu] = useState<OmnibarCtxMenuState | null>(null);

  /** 当前路径中是否存在软链接目录段（从 breadcrumb 条目判断） */
  const hasPathSymlinks = useMemo(
    () => breadcrumbs.some(e => e.is_symlink),
    [breadcrumbs],
  );

  useEffect(() => {
    if (!isEditing) setInputValue(currentPath);
  }, [currentPath, isEditing]);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  useEffect(() => {
    setOmnibarCtxMenu(null);
  }, [currentPath]);

  const handleEditContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setOmnibarCtxMenu({ x: e.clientX, y: e.clientY });
  }, []);

  const omnibarCtxMenuItems: ContextMenuItem[] = hasPathSymlinks
    ? [{
      label: t("omnibar.flatten_symlinks"),
      icon: "link",
      action: async () => {
        try {
          const resolved: string = await invoke("realpath", { path: currentPath });
          onNavigate(resolved);
        } catch {
          // realpath failed — do nothing
        }
      },
    }]
    : [];

  const handleSubmit = () => {
    setIsEditing(false);
    const trimmed = inputValue.trim();
    if (!trimmed) return;

    if (trimmed.startsWith("/") || trimmed.startsWith("~") || trimmed.includes("/")) {
      onNavigate(trimmed);
    } else {
      onSearch(trimmed);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") handleSubmit();
    if (e.key === "Escape") {
      setIsEditing(false);
      setInputValue(currentPath);
    }
  };

  return (
    <div className={`omnibar ${isEditing ? "editing" : ""}`}>
      {isEditing ? (
        <div className="omnibar-input-wrapper">
          <Icon
            name={inputValue.startsWith("/") ? "folder_open" : "search"}
            className="omnibar-icon"
          />
          <input
            ref={inputRef}
            type="text"
            className="omnibar-input"
            value={inputValue}
            onChange={(e) => setInputValue(e.target.value)}
            onKeyDown={handleKeyDown}
            onBlur={() => setIsEditing(false)}
            placeholder={t("omnibar.placeholder")}
          />
        </div>
      ) : (
        <div className="omnibar-breadcrumbs">
          <Breadcrumbs
            entries={breadcrumbs}
            onNavigate={onNavigate}
          />
          <IconButton
            variant="standard"
            className="omnibar-trigger"
            onClick={() => setIsEditing(true)}
            onContextMenu={handleEditContextMenu}
            title={t("omnibar.button_tip")}
          >
            <Icon name="edit" className="edit-icon" />
          </IconButton>
        </div>
      )}

      {omnibarCtxMenu && omnibarCtxMenuItems.length > 0 && (
        <ContextMenu
          x={omnibarCtxMenu.x}
          y={omnibarCtxMenu.y}
          items={omnibarCtxMenuItems}
          onClose={() => setOmnibarCtxMenu(null)}
        />
      )}
    </div>
  );
};
