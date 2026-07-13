import { useState, useEffect, useCallback, useRef } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { showToast } from "./utils/toast";
import { t, setLocale, getLocale, type Locale } from "./i18n";
import "./index.css";
import { ToastContainer } from "react-toastify";
import "react-toastify/dist/ReactToastify.css";
import "./utils/toast.css";
import { ClipboardProvider, useClipboard } from "./contexts/ClipboardContext";
import { ThemeService } from "./services/ThemeService";
import { NavigationRail } from "./components/NavigationRail";
import { Sidebar } from "./components/Sidebar";
import { Icon } from "./components/Icon";
import { IconButton } from "./components/IconButton";
import { ContextMenu } from "./components/ContextMenu";
import type { ContextMenuItem } from "./components/ContextMenu";
import { SettingsDialog } from "./components/SettingsDialog";
import { TerminalPane } from "./components/TerminalPane";
import type { IFile } from "./types/files";
import { Dialog } from "./components/Dialog";
import { Button } from "./components/Button";
import { OutlinedTextField } from "./components/md";
import { TabBar } from "./components/TabBar";
import { ExplorerTab } from "./components/ExplorerTab";
import { OpenWithDialog } from "./components/OpenWithDialog";
import { PropertiesDialog } from "./components/PropertiesDialog";
import { useLocalStorage } from "./hooks/useLocalStorage";
import { importFiles } from "./utils/fileOperations";
import { DragOverProvider, useSetDragOver } from "./utils/dnd";
import {
  trashFile,
  pasteFiles,
  extractFile,
  openFile,
} from "./utils/fileOperations";
import { NameInputDialog } from "./components/NameInputDialog";
import { ConflictDialog } from "./components/ConflictDialog";
import {
  generateSafeName,
  splitNameExt,
} from "./utils/fileConflict";
import { useTabs } from "./hooks/useTabs";
import { useContextMenu } from "./hooks/useContextMenu";
import { useRenameDialog } from "./hooks/useRenameDialog";
import { useConflictDialog } from "./hooks/useConflictDialog";
import { useCreateDialog } from "./hooks/useCreateDialog";
import { useDeviceActions } from "./hooks/useDeviceActions";

function AppContent() {
  const {
    tabs,
    activeTabId,
    currentPath,
    handleAddTab,
    handleCloseTab,
    handleSwitchTab,
    handleSidebarNavigate,
    refreshActiveTab,
  } = useTabs();

  const setDragOverPath = useSetDragOver();
  const lastDropTimeRef = useRef(0);

  const {
    contextMenu,
    bgMenuItems,
    deviceContextMenu,
    handleContextMenu,
    handleDeviceContextMenu,
    handleBgMenuItems,
    closeContextMenu,
    closeDeviceContextMenu,
  } = useContextMenu();

  const {
    renameDialogOpen,
    setRenameDialogOpen,
    newName,
    setNewName,
    handleRename,
    openRenameDialog,
  } = useRenameDialog(refreshActiveTab);

  const {
    singleConflict,
    setSingleConflict,
    multiConflict,
    setMultiConflict,
    handleConflictDialog,
  } = useConflictDialog();

  const {
    createDialog,
    setCreateDialog,
    handleCreateDialog,
  } = useCreateDialog();

  const {
    handleDeviceMount,
    handleDeviceUnmount,
    handleDeviceEject,
  } = useDeviceActions();

  /** Tab 右键菜单状态（"在新窗口中打开"等操作） */
  const [tabContextMenu, setTabContextMenu] = useState<{
    x: number;
    y: number;
    tabId: number;
  } | null>(null);

  const handleTabContextMenu = useCallback((e: React.MouseEvent, tabId: number) => {
    e.preventDefault();
    e.stopPropagation();
    setTabContextMenu({ x: e.clientX, y: e.clientY, tabId });
  }, []);

  const closeTabContextMenu = useCallback(() => setTabContextMenu(null), []);

  const [terminalOpen, setTerminalOpen] = useState(false);
  const [terminalCwd, setTerminalCwd] = useState<string | undefined>(undefined);

  const openTerminalAt = useCallback((path: string) => {
    setTerminalCwd(path);
    setTerminalOpen(true);
  }, []);

  const [propertiesDialogOpen, setPropertiesDialogOpen] = useState(false);
  const [propertiesFile, setPropertiesFile] = useState<IFile | null>(null);

  const [openWithFile, setOpenWithFile] = useState<IFile | null>(null);

  const { clipboard, copy, cut, clear: clearClipboard } = useClipboard();

  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);

  const [showHiddenFiles, setShowHiddenFiles] = useLocalStorage<boolean>(
    "settings.showHiddenFiles",
    true,
  );
  const [iconSize, setIconSize] = useLocalStorage<number>(
    "settings.iconSize",
    64,
  );
  const [viewMode, setViewMode] = useLocalStorage<"grid" | "list">(
    "settings.viewMode",
    "grid",
  );
  const [filledIcons, setFilledIcons] = useLocalStorage<boolean>(
    "settings.filledIcons",
    false,
  );
  const [customCssPath, setCustomCssPath] = useState<string>("");
  const [locale, setLocaleState] = useLocalStorage<Locale>(
    "settings.locale",
    getLocale(),
  );
  const [marqueeEnabled, setMarqueeEnabled] = useLocalStorage<boolean>(
    "settings.marqueeEnabled",
    true,
  );

  useEffect(() => {
    setLocale(locale);
  }, [locale]);

  const handleLoadCustomCss = async (path: string) => {
    try {
      const css = await window.electron.readFile(path);
      if (css) {
        let style = document.getElementById("custom-user-css");
        if (!style) {
          style = document.createElement("style");
          style.id = "custom-user-css";
          document.head.appendChild(style);
        }
        style.textContent = css;
        setCustomCssPath(path);
        localStorage.setItem("customCssPath", path);
      }
    } catch (err) {
      console.error("Failed to load custom css", err);
    }
  };

  const handleImportCss = async () => {
    const path = await window.electron.openFileDialog();
    if (path) {
      handleLoadCustomCss(path);
    }
  };

  const hasInitialized = useRef(false);

  useEffect(() => {
    if (hasInitialized.current) return;
    hasInitialized.current = true;

    ThemeService.loadTheme();
    ThemeService.init();

    // useTabs hook 内部调用 invoke("ready") 触发初始状态推送
    // 无需额外 init 逻辑

    const storedCssPath = localStorage.getItem("customCssPath");
    if (storedCssPath) {
      handleLoadCustomCss(storedCssPath);
    }
  }, []);

  /**
   * 监听 Tauri 后端发来的 "navigate-to" 事件。
   * 
   * 当后端通过 create_window() 创建新窗口并 emit 路径时，
   * 前端据此创建对应的 tab。
   * 
   * 仅在 Tauri 环境下注册（浏览器中由 electron mock 处理）。
   */
  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | undefined;
    const setup = async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const ul = await listen<string>("navigate-to", (event) => {
        handleAddTab(event.payload);
      });
      unlisten = ul;
    };
    setup();
    return () => { unlisten?.(); };
  }, [handleAddTab]);

  /**
   * 监听 Tauri 原生拖放事件（外部→内部）。
   *
   * 当文件从桌面/其他应用拖入窗口时，Tauri 触发 onDragDropEvent。
   * - over 事件：用坐标检测鼠标下方的文件夹元素
   * - drop 事件：导入文件到目标文件夹或当前目录
   *
   * 仅在 Tauri 环境下注册。
   */
  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | undefined;
    const setup = async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const ul = await win.onDragDropEvent((event) => {
          const payload = event.payload;
          console.log("[dnd] onDragDropEvent:", payload.type);
          switch (payload.type) {
            case "over": {
              // 用坐标查找鼠标下方的文件夹元素
              const pos = payload.position;
              const el = document.elementFromPoint(pos.x, pos.y);
              const folderItem = el?.closest('[data-droppable-id^="folder:"]');
              if (folderItem) {
                const droppableId = folderItem.getAttribute("data-droppable-id");
                const folderPath = droppableId?.replace("folder:", "");
                if (folderPath) {
                  setDragOverPath(folderPath);
                  sessionStorage.setItem("hnfm-dragover-folder", folderPath);
                }
              } else {
                setDragOverPath(null);
                sessionStorage.removeItem("hnfm-dragover-folder");
              }
              break;
            }
            case "drop": {
              // 防重入：1秒内忽略重复 drop
              const now = Date.now();
              if (now - lastDropTimeRef.current < 1000) {
                console.log("[dnd] drop ignored (duplicate)");
                break;
              }
              lastDropTimeRef.current = now;

              setDragOverPath(null);
              let paths = payload.paths;
              console.log("[dnd] drop paths:", paths);

              // 如果没有路径，检查跨窗口拖放数据
              if (paths.length === 0) {
                const crossWindowData = sessionStorage.getItem("hnfm-cross-window-drag");
                if (crossWindowData) {
                  try {
                    const { files } = JSON.parse(crossWindowData);
                    paths = files.map((f: { path: string }) => f.path);
                    console.log("[dnd] cross-window drag data:", paths);
                  } catch (e) {
                    console.error("[dnd] failed to parse cross-window drag data:", e);
                  }
                  sessionStorage.removeItem("hnfm-cross-window-drag");
                }
              }

              if (paths.length === 0) {
                console.log("[dnd] no paths to import");
                break;
              }

              const targetFolder = sessionStorage.getItem("hnfm-dragover-folder");
              const currentPath = sessionStorage.getItem("hnfm-current-path") || "/";
              const target = targetFolder || currentPath;
              console.log("[dnd] importing to:", target);
              try {
                importFiles(
                  paths.map((p: string) => ({ path: p })),
                  target,
                  () => refreshActiveTab(),
                ).then(() => {
                  showToast(t("toast.imported_files", paths.length), "success");
                }).catch((e) => {
                  console.error("[dnd] importFiles failed:", e);
                  showToast(t("error.import_failed"), "error");
                });
              } catch (e) {
                console.error("[dnd] importFiles exception:", e);
                showToast(t("error.import_failed"), "error");
              }
              sessionStorage.removeItem("hnfm-dragover-folder");
              break;
            }
            case "leave":
              console.log("[dnd] leave");
              setDragOverPath(null);
              sessionStorage.removeItem("hnfm-dragover-folder");
              break;
          }
        });
        unlisten = ul;
      } catch (e) {
        console.error("[dnd] Failed to setup onDragDropEvent:", e);
      }
    };
    setup();
    return () => { unlisten?.(); };
  }, [refreshActiveTab]);

  /**
   * 监听跨窗口拖放事件。
   *
   * 当用户从一个窗口拖拽文件到另一个窗口时，
   * 源窗口通过 emit("dnd:drag-start") 发送拖拽数据，
   * 目标窗口通过 listen("dnd:drag-start") 接收数据并存储。
   *
   * 仅在 Tauri 环境下注册。
   */
  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | undefined;
    const setup = async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const ul = await listen<{ files: { path: string; name: string; isDirectory: boolean }[]; sourcePath: string }>("dnd:drag-start", (event) => {
        console.log("[dnd] cross-window drag-start received:", event.payload);
        sessionStorage.setItem("hnfm-cross-window-drag", JSON.stringify(event.payload));
      });
      unlisten = ul;
    };
    setup();
    return () => { unlisten?.(); };
  }, []);

  useEffect(() => {
    const handler = (e: WheelEvent) => {
      const target = e.target as Node;
      const menu = document.querySelector('.context-menu');
      if (menu) {
        if (menu.contains(target)) return;
        e.preventDefault();
        return;
      }
      const openDialog = document.querySelector('md-dialog[open]');
      if (openDialog) {
        if (openDialog.contains(target)) return;
        e.preventDefault();
      }
    };
    window.addEventListener('wheel', handler, { passive: false });
    return () => window.removeEventListener('wheel', handler);
  }, []);

  const toggleTerminal = () => {
    setTerminalOpen((prev) => !prev);
  };

  const handleOpenWithFile = useCallback((file: IFile) => {
    setOpenWithFile(file);
  }, []);

  const handlePropertiesFile = useCallback((file: IFile) => {
    setPropertiesFile(file);
    setPropertiesDialogOpen(true);
  }, []);

  const handleCopy = (file: IFile) => {
    copy([file]);
    closeContextMenu();
  };

  const handleCut = (file: IFile) => {
    cut([file]);
    closeContextMenu();
  };

  const handlePaste = async () => {
    if (!clipboard || clipboard.files.length === 0) return;

    await pasteFiles(
      clipboard.files,
      clipboard.operation,
      currentPath,
      [],
      clipboard.operation === "cut" ? clearClipboard : undefined,
      refreshActiveTab,
    );
    closeContextMenu();
  };

  const menuItems: ContextMenuItem[] = (() => {
    const item = contextMenu?.item;
    if (item) {
      const items: ContextMenuItem[] = [
        {
          label: t("context_menu.open"),
          icon: "open_in_new",
          action: () => {
            openFile(item.path);
            closeContextMenu();
          },
        },
        {
          label: t("context_menu.open_terminal"),
          icon: "terminal",
          action: () => openTerminalAt(item.path),
        },
        { divider: true, label: "", action: () => {} },
        {
          label: t("context_menu.copy"),
          icon: "content_copy",
          action: () => handleCopy(item),
        },
        {
          label: t("context_menu.cut"),
          icon: "content_cut",
          action: () => handleCut(item),
        },
        {
          label: t("context_menu.delete"),
          icon: "delete",
          action: () =>
            trashFile(item.path, refreshActiveTab),
        },
        {
          label: t("context_menu.extract_here"),
          icon: "unarchive",
          action: () => {
            extractFile(item.path, refreshActiveTab);
          },
        },
        {
          label: t("context_menu.rename"),
          icon: "edit",
          action: () => {
            openRenameDialog(item);
            closeContextMenu();
          },
        },
      ];

      const specialItems: ContextMenuItem[] = [];
      if (item.symlinkTarget && item.mime !== 'inode/symlink') {
        specialItems.push({
          label: t("symlink.go_to_target"),
          icon: "arrow_forward",
          action: () => {
            if (item.isDirectory) {
              handleSidebarNavigate(item.symlinkTarget!);
            } else {
              const parent = item.symlinkTarget!.substring(0, item.symlinkTarget!.lastIndexOf("/"));
              handleSidebarNavigate(parent || "/");
            }
            closeContextMenu();
          },
        });
      }
      if (item.isMountpoint && item.mountSource) {
        const isRealDevice = item.mountSource.startsWith("/dev/") &&
          !["devtmpfs", "tmpfs", "sysfs", "proc", "hugetlbfs", "mqueue", "selinuxfs", "debugfs", "fusectl", "securityfs", "pstore", "bpf", "cgroup2", "configfs"].includes(
            item.mountSource.split("/").pop() || ""
          );
        if (isRealDevice) {
          specialItems.push({
            label: t("mountpoint.go_to_source"),
            icon: "hard_drive",
            action: () => {
              const parent = item.mountSource!.substring(0, item.mountSource!.lastIndexOf("/"));
              handleSidebarNavigate(parent || "/");
              closeContextMenu();
            },
          });
        }
      }
      if (item.mime === 'inode/blockdevice' && item.isExternal) {
        const devPath = item.devicePath || item.path;
        if (item.isMountable) {
          if (item.mountedAt) {
            specialItems.push({
              label: t("device.unmount"),
              icon: "eject",
              action: () => {
                handleDeviceUnmount(devPath);
                closeContextMenu();
              },
            });
          } else {
            specialItems.push({
              label: t("device.mount"),
              icon: "hard_drive",
              action: () => {
                handleDeviceMount(devPath);
                closeContextMenu();
              },
            });
          }
        }
        if (!item.parentDisk && !item.isMountable) {
          specialItems.push({
            label: t("device.eject"),
            icon: "power_settings_new",
            action: () => {
              handleDeviceEject(devPath);
              closeContextMenu();
            },
          });
        }
      }
      if (specialItems.length > 0) {
        items.push({ divider: true, label: "", action: () => {} }, ...specialItems);
      }

      items.push(
        { divider: true, label: "", action: () => {} },
        {
          label: t("context_menu.open_with"),
          icon: "apps",
          action: () => {
            setOpenWithFile(item);
            closeContextMenu();
          },
        },
        {
          label: t("context_menu.properties"),
          icon: "info",
          action: () => {
            setPropertiesFile(item);
            setPropertiesDialogOpen(true);
            closeContextMenu();
          },
        },
      );

      return items;
    }
    return (
      bgMenuItems ??
      [{ label: t("context_menu.paste"), icon: "content_paste", action: handlePaste }].filter(
        (menuItem) => {
          if (
            menuItem.label === t("context_menu.paste") &&
            (!clipboard || clipboard.files.length === 0)
          )
            return false;
          return true;
        },
      )
    );
  })();

  return (
    <div className="app-shell" onClick={() => { closeContextMenu(); closeTabContextMenu(); }}>
      <NavigationRail
        items={[
          {
            icon: <Icon name="dashboard" />,
            activeIcon: <Icon name="dashboard" filled />,
            label: "Dashboard",
            active: currentPath === "app://dashboard",
            onClick: () => {
              handleSidebarNavigate("app://dashboard");
            },
          },
          {
            icon: <Icon name="home" />,
            activeIcon: <Icon name="home" filled />,
            label: "Home",
            active: currentPath.startsWith("/home"),
            onClick: async () => {
              const home = await window.electron.getHomePath();
              handleSidebarNavigate(home);
            },
          },
          {
            icon: <Icon name="folder" />,
            activeIcon: <Icon name="folder" filled />,
            label: "Files",
            active: currentPath === "/",
            onClick: () => handleSidebarNavigate("/"),
          },
          {
            icon: <Icon name="terminal" />,
            activeIcon: <Icon name="terminal" filled />,
            label: "Terminal",
            active: terminalOpen,
            onClick: toggleTerminal,
          },
          {
            icon: <Icon name="settings" />,
            activeIcon: <Icon name="settings" filled />,
            label: "Settings",
            onClick: () => setSettingsDialogOpen(true),
          },
        ]}
      />

      <Sidebar
        onNavigate={handleSidebarNavigate}
        currentPath={currentPath}
        onDeviceContextMenu={handleDeviceContextMenu}
        onDeviceMount={handleDeviceMount}
        onDeviceUnmount={handleDeviceUnmount}
        onDeviceEject={handleDeviceEject}
        marqueeEnabled={marqueeEnabled}
      />

      <main className="main-content">
        <header className="tab-header-bar">
          <TabBar
            tabs={tabs}
            activeTabId={activeTabId}
            onTabClick={handleSwitchTab}
            onTabClose={handleCloseTab}
            onNewTab={() => handleAddTab()}
            onTabContextMenu={handleTabContextMenu}
          />
        </header>

        <div className="content-area">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              style={{
                display: tab.id === activeTabId ? "block" : "none",
                height: "100%",
              }}
            >
              <ExplorerTab
                tabId={tab.id}
                isActive={tab.id === activeTabId}
                initialPath={tab.path}
                onPathChange={handleSidebarNavigate}
                onContextMenu={handleContextMenu}
                onBgMenuItems={handleBgMenuItems}
                onOpenWithFile={handleOpenWithFile}
                onPropertiesFile={handlePropertiesFile}
                onOpenTerminalAt={openTerminalAt}
                onCreateDialog={handleCreateDialog}
                onConflictDialog={handleConflictDialog}
                showHiddenFiles={showHiddenFiles}
                iconSize={iconSize}
                viewMode={viewMode}
                filledIcons={filledIcons}
                onMountDevice={handleDeviceMount}
                marqueeEnabled={marqueeEnabled}
                onRefresh={refreshActiveTab}
              />
            </div>
          ))}
          {tabs.length === 0 && (
            <div className="empty-state">
              <div className="empty-state-content">
                <Icon name="tab" size={48} />
                <p>{t("empty.no_tabs")}</p>
                <Button onClick={() => handleAddTab()}>{t("empty.open_new_tab")}</Button>
              </div>
            </div>
          )}
        </div>

        {terminalOpen && (
          <div className="terminal-panel">
            <div className="terminal-panel-header">
              <span style={{ fontSize: "12px", fontWeight: 500 }}>
                {t("terminal.title")}
              </span>
              <div style={{ flex: 1 }} />
              <IconButton
                onClick={() => setTerminalOpen(false)}
                style={{ width: 24, height: 24 }}
              >
                <Icon name="close" size={16} />
              </IconButton>
            </div>
            <div style={{ flex: 1, position: "relative" }}>
              <TerminalPane
                cwd={
                  terminalCwd ||
                  tabs.find((t) => t.id === activeTabId)?.path ||
                  undefined
                }
              />
            </div>
          </div>
        )}

        {contextMenu && (
          <ContextMenu
            x={contextMenu.x}
            y={contextMenu.y}
            items={menuItems}
            onClose={closeContextMenu}
          />
        )}

        {deviceContextMenu && (
          <ContextMenu
            x={deviceContextMenu.x}
            y={deviceContextMenu.y}
            items={(() => {
              const d = deviceContextMenu.device;
              const items: ContextMenuItem[] = [];
              items.push({
                label: t("device.go_to_source"),
                icon: "hard_drive",
                action: () => {
                  handleSidebarNavigate("/dev");
                  closeDeviceContextMenu();
                },
              });
              if (d.mounted) {
                items.push({
                  label: t("device.unmount"),
                  icon: "eject",
                  action: () => {
                    handleDeviceUnmount(d.devicePath);
                    closeDeviceContextMenu();
                  },
                });
                if (d.type !== 'part' && (d.hotplug || d.rm || d.tran === 'usb')) {
                  items.push({
                    label: t("device.eject"),
                    icon: "power_settings_new",
                    action: () => {
                      handleDeviceEject(d.devicePath);
                      closeDeviceContextMenu();
                    },
                  });
                }
              } else {
                items.push({
                  label: t("device.mount"),
                  icon: "hard_drive",
                  action: () => {
                    handleDeviceMount(d.devicePath);
                    closeDeviceContextMenu();
                  },
                });
              }
              return items;
            })()}
            onClose={closeDeviceContextMenu}
          />
        )}

        {tabContextMenu && (() => {
          const tab = tabs.find((t) => t.id === tabContextMenu.tabId);
          const tabItems: ContextMenuItem[] = [
            {
              label: t("context_menu.open_in_new_window"),
              icon: "open_in_new",
              action: () => {
                if (tab) handleAddTab(tab.path);
                closeTabContextMenu();
              },
            },
          ];
          return (
            <ContextMenu
              x={tabContextMenu.x}
              y={tabContextMenu.y}
              items={tabItems}
              onClose={closeTabContextMenu}
            />
          );
        })()}

        <Dialog
          title={t("dialog.rename.title")}
          open={renameDialogOpen}
          onClose={() => setRenameDialogOpen(false)}
          actions={
            <>
              <Button variant="text" onClick={() => setRenameDialogOpen(false)}>
                {t("dialog.rename.cancel")}
              </Button>
              <Button onClick={handleRename}>{t("dialog.rename.confirm")}</Button>
            </>
          }
        >
          <OutlinedTextField
            label={t("dialog.rename.title")}
            value={newName}
            onInput={(e) => setNewName((e.target as HTMLInputElement).value)}
            onKeyDown={(e) => {
              if ((e as React.KeyboardEvent).key === "Enter") handleRename();
            }}
            style={{ width: "100%" }}
          />
        </Dialog>

        {createDialog && (
          <NameInputDialog
            title={createDialog.type === "folder" ? t("dialog.create.folder") : t("dialog.create.file")}
            defaultName={createDialog.defaultName}
            isDir={createDialog.type === "folder"}
            existingNames={createDialog.existingNames}
            onConfirm={(name) => {
              const r = createDialog.resolve;
              setCreateDialog(null);
              r(name);
            }}
            onCancel={() => {
              const r = createDialog.resolve;
              setCreateDialog(null);
              r(null);
            }}
          />
        )}

        {singleConflict &&
          (() => {
            const c = singleConflict;
            const { base, ext } = splitNameExt(
              c.conflict.entry.name,
              c.conflict.isDir,
            );
            const existingSet = new Set(c.existingNames);
            const safeName = generateSafeName(
              base,
              ext,
              existingSet,
              c.conflict.isDir,
            );
            return (
              <NameInputDialog
                title={t("dialog.conflict.single_title")}
                defaultName={safeName}
                isDir={c.conflict.isDir}
                existingNames={c.existingNames}
                sourcePath={c.sourcePath}
                operation={c.operation}
                destDir={c.destDir}
                onConfirm={(name) => {
                  const renames = new Map<string, string>();
                  renames.set(c.conflict.entry.name, name);
                  const resolve = c.resolve;
                  setSingleConflict(null);
                  resolve({ action: "auto-rename", renames });
                }}
                onCancel={() => {
                  const resolve = c.resolve;
                  setSingleConflict(null);
                  resolve({ action: "skip" });
                }}
              />
            );
          })()}

        {multiConflict && (
          <ConflictDialog
            conflicts={multiConflict.conflicts}
            destDir={multiConflict.destDir}
            existingNames={multiConflict.existingNames}
            sourcePath={multiConflict.sourcePath}
            operation={multiConflict.operation}
            onConfirm={(result) => {
              const resolve = multiConflict.resolve;
              setMultiConflict(null);
              resolve(result);
            }}
            onCancel={() => {
              const resolve = multiConflict.resolve;
              setMultiConflict(null);
              resolve({ action: "skip" });
            }}
          />
        )}

        <PropertiesDialog
          open={propertiesDialogOpen}
          onClose={() => setPropertiesDialogOpen(false)}
          file={propertiesFile}
        />
        {openWithFile && (
          <OpenWithDialog
            open={!!openWithFile}
            path={openWithFile.path}
            onClose={() => setOpenWithFile(null)}
            onSelect={async (exec, desktopFile) => {
              if (openWithFile) {
                const result = await window.electron.openWith(
                  exec,
                  openWithFile.path,
                  desktopFile,
                );
                if (result !== true) {
                  showToast(t("toast.launch_failed", exec, result), "error");
                }
              }
              setOpenWithFile(null);
            }}
          />
        )}

        <SettingsDialog
          open={settingsDialogOpen}
          onClose={() => setSettingsDialogOpen(false)}
          showHiddenFiles={showHiddenFiles}
          onToggleHiddenFiles={() => setShowHiddenFiles(!showHiddenFiles)}
          iconSize={iconSize}
          onIconSizeChange={setIconSize}
          viewMode={viewMode}
          onViewModeChange={setViewMode}
          filledIcons={filledIcons}
          onToggleFilledIcons={() => setFilledIcons(!filledIcons)}
          customCssPath={customCssPath}
          onImportCss={handleImportCss}
          locale={locale}
          onLocaleChange={setLocaleState}
          marqueeEnabled={marqueeEnabled}
          onToggleMarquee={() => setMarqueeEnabled(!marqueeEnabled)}
        />
      </main>
    </div>
  );
}

function App() {
  return (
    <>
      <ClipboardProvider>
        <DragOverProvider>
          <AppContent />
        </DragOverProvider>
      </ClipboardProvider>
      <ToastContainer
        position="bottom-right"
        autoClose={5000}
        hideProgressBar={false}
        newestOnTop={false}
        closeOnClick
        pauseOnHover
        theme="dark"
        limit={5}
        style={{ zIndex: 2000 }}
      />
    </>
  );
}

export default App;
