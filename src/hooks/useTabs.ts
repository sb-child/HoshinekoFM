import { useState, useCallback } from 'react';
import { t } from '../i18n';

interface TabState {
  id: string;
  title: string;
  path: string;
  version: number;
  pendingSelectFile?: string;
}

export function useTabs() {
  const [tabs, setTabs] = useState<TabState[]>([]);
  const [activeTabId, setActiveTabId] = useState<string>("");

  const handleAddTab = useCallback((path?: string) => {
    const newTabId = Date.now().toString();
    const currentPath = path || "/";
    const newTab: TabState = {
      id: newTabId,
      title: "New Tab",
      path: currentPath,
      version: 0,
    };
    setTabs((prev) => [...prev, newTab]);
    setActiveTabId(newTabId);
  }, []);

  const handleCloseTab = useCallback(
    (id: string) => {
      setTabs((prevTabs) => {
        const newTabs = prevTabs.filter((t) => t.id !== id);
        if (newTabs.length > 0) {
          setActiveTabId(newTabs[newTabs.length - 1].id);
        } else {
          setActiveTabId("");
        }
        return newTabs;
      });
    },
    [],
  );

  const handleTabPathUpdate = useCallback((id: string, path: string) => {
    const folderName = path.split("/").pop() || path;
    setTabs((prev) =>
      prev.map((t) => {
        if (t.id === id) return { ...t, path, title: folderName };
        return t;
      }),
    );
  }, []);

  const handleSidebarNavigate = useCallback(
    (path: string, selectFileName?: string) => {
      setTabs((prev) => {
        if (prev.length === 0) {
          const newTabId = Date.now().toString();
          const newTab: TabState = {
            id: newTabId,
            title: t("tab.new_tab"),
            path,
            version: 0,
            pendingSelectFile: selectFileName,
          };
          setActiveTabId(newTabId);
          return [newTab];
        }
        return prev.map((t) => {
          if (t.id === activeTabId) {
            return { ...t, path, pendingSelectFile: selectFileName };
          }
          return t;
        });
      });
    },
    [activeTabId],
  );

  const handleScrollToComplete = useCallback(() => {
    setTabs((prev) =>
      prev.map((t) => (t.id === activeTabId ? { ...t, pendingSelectFile: undefined } : t)),
    );
  }, [activeTabId]);

  const refreshActiveTab = useCallback(() => {
    setTabs((prev) =>
      prev.map((t) =>
        t.id === activeTabId ? { ...t, version: t.version + 1 } : t,
      ),
    );
  }, [activeTabId]);

  const currentPath = tabs.find((t) => t.id === activeTabId)?.path || "";

  return {
    tabs,
    setTabs,
    activeTabId,
    setActiveTabId,
    currentPath,
    handleAddTab,
    handleCloseTab,
    handleTabPathUpdate,
    handleSidebarNavigate,
    handleScrollToComplete,
    refreshActiveTab,
  };
}
