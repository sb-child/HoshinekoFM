import { useState, useCallback } from 'react';
import type { IFile, AllDevice } from '../types/files';
import type { ContextMenuItem } from '../components/ContextMenu';

export function useContextMenu() {
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    item: IFile | null;
  } | null>(null);
  const [bgMenuItems, setBgMenuItems] = useState<ContextMenuItem[] | null>(null);
  const [deviceContextMenu, setDeviceContextMenu] = useState<{
    x: number;
    y: number;
    device: AllDevice;
  } | null>(null);

  const handleContextMenu = useCallback(
    (e: React.MouseEvent, file: IFile | null) => {
      e.preventDefault();
      e.stopPropagation();
      setContextMenu({ x: e.clientX, y: e.clientY, item: file });
    },
    [],
  );

  const handleDeviceContextMenu = useCallback(
    (e: React.MouseEvent, device: AllDevice) => {
      e.preventDefault();
      e.stopPropagation();
      setDeviceContextMenu({ x: e.clientX, y: e.clientY, device });
    },
    [],
  );

  const handleBgMenuItems = useCallback((items: ContextMenuItem[]) => {
    setBgMenuItems(items);
  }, []);

  const closeContextMenu = useCallback(() => setContextMenu(null), []);
  const closeDeviceContextMenu = useCallback(() => setDeviceContextMenu(null), []);

  return {
    contextMenu,
    setContextMenu,
    bgMenuItems,
    deviceContextMenu,
    setDeviceContextMenu,
    handleContextMenu,
    handleDeviceContextMenu,
    handleBgMenuItems,
    closeContextMenu,
    closeDeviceContextMenu,
  };
}
