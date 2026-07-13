import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { TabsPayload, NavStatePayload, TabInfo } from "../types/tauriEvents";

export interface TabState {
  id: number;
  title: string;
  path: string;
}

/**
 * 事件驱动的 Tab 管理 hook。
 *
 * 后端是 Tab 状态的唯一来源。前端通过 invoke 发意图，
 * 后端通过 `hf:tabs` event 推送 Tab 列表变化。
 */
export function useTabs() {
  const [tabs, setTabs] = useState<TabState[]>([]);
  const [activeTabId, setActiveTabId] = useState<number>(0);
  const [currentPath, setCurrentPath] = useState("");
  const currentPathRef = useRef<string>("");
  const readyRef = useRef(false);
  const unlistens = useRef<UnlistenFn[]>([]);

  /** 持久化 currentPath 供 DnD 等模块使用 */
  useEffect(() => {
    currentPathRef.current = currentPath;
  }, [currentPath]);

  useEffect(() => {
    sessionStorage.setItem("hnfm-current-path", currentPath);
  }, [currentPath]);

  /** 映射后端 TabInfo → 前端 TabState */
  const mapTabInfo = useCallback((info: TabInfo): TabState => {
    let path: string;
    if (info.nav_target.Dashboard !== undefined) {
      path = "app://dashboard";
    } else if (info.nav_target.Filesystem) {
      path = info.nav_target.Filesystem;
    } else {
      path = "/";
    }
    return { id: info.id, title: info.title, path };
  }, []);

  /** 挂载：调用 ready + 注册事件监听 */
  useEffect(() => {
    if (readyRef.current) return;
    readyRef.current = true;

    const setup = async () => {
      // 注册事件监听
      const ul1 = await listen<TabsPayload>("hf:tabs", (event) => {
        const payload = event.payload;
        const mapped = payload.tabs.map(mapTabInfo);
        setTabs(mapped);
        if (payload.active_tab_id > 0) {
          setActiveTabId(payload.active_tab_id);
          const active = payload.tabs.find((t) => t.id === payload.active_tab_id);
          if (active) {
            const path = active.nav_target.Filesystem ?? "app://dashboard";
            setCurrentPath(path);
            currentPathRef.current = path;
          }
        }
      });

      const ul2 = await listen<NavStatePayload>("hf:nav-state", (event) => {
        const payload = event.payload;
        if (payload.tab_id === activeTabId || activeTabId === 0) {
          const path = payload.target.Filesystem ?? "app://dashboard";
          setCurrentPath(path);
          currentPathRef.current = path;
        }
      });

      unlistens.current = [ul1, ul2];

      // 告诉后端：就绪
      try {
        await invoke("ready");
      } catch (e) {
        console.error("[useTabs] ready failed:", e);
      }
    };

    setup();
    return () => {
      unlistens.current.forEach((fn) => fn());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  /** 创建新 tab */
  const handleAddTab = useCallback(async (path?: string) => {
    try {
      await invoke("new_tab", { path: path ?? null });
    } catch (e) {
      console.error("[useTabs] new_tab failed:", e);
    }
  }, []);

  /** 关闭 tab */
  const handleCloseTab = useCallback(async (id: number) => {
    try {
      await invoke("close_tab", { tabId: id });
    } catch (e) {
      console.error("[useTabs] close_tab failed:", e);
    }
  }, []);

  /** 切换 tab */
  const handleSwitchTab = useCallback(async (id: number) => {
    try {
      await invoke("switch_tab", { tabId: id });
    } catch (e) {
      console.error("[useTabs] switch_tab failed:", e);
    }
  }, []);

  /** 导航到指定路径 */
  const handleSidebarNavigate = useCallback(
    async (path: string) => {
      try {
        const target = path === "app://dashboard"
          ? { Dashboard: null }
          : { Filesystem: path };
        await invoke("nav_to", { tabId: activeTabId, target });
      } catch (e) {
        console.error("[useTabs] nav_to failed:", e);
      }
    },
    [activeTabId],
  );

  /** 刷新活跃 tab 的文件列表（通知 watcher 重新读取） */
  const refreshActiveTab = useCallback(async () => {
    try {
      await invoke("refresh_tab");
    } catch (e) {
      console.error("[useTabs] refresh failed:", e);
    }
  }, []);

  return {
    tabs,
    activeTabId,
    setActiveTabId,
    currentPath,
    handleAddTab,
    handleCloseTab,
    handleSwitchTab,
    handleSidebarNavigate,
    refreshActiveTab,
    currentPathRef,
  };
}
