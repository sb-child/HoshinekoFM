import { useState, useCallback } from 'react';
import { renameFile as renameFileOp } from '../utils/fileOperations';
import type { IFile } from '../types/files';

export function useRenameDialog(onTabRefresh: () => void) {
  const [renameDialogOpen, setRenameDialogOpen] = useState(false);
  const [renameFile, setRenameFile] = useState<IFile | null>(null);
  const [newName, setNewName] = useState("");

  const openRenameDialog = useCallback((file: IFile) => {
    setRenameFile(file);
    setNewName(file.name);
    setRenameDialogOpen(true);
  }, []);

  const handleRename = useCallback(async () => {
    if (renameFile && newName && newName !== renameFile.name) {
      const lastSlashIndex = renameFile.path.lastIndexOf("/");
      const parentDir = renameFile.path.substring(0, lastSlashIndex);
      await renameFileOp(renameFile.path, `${parentDir}/${newName}`, onTabRefresh);
    }
    setRenameDialogOpen(false);
    setRenameFile(null);
  }, [renameFile, newName, onTabRefresh]);

  return {
    renameDialogOpen,
    setRenameDialogOpen,
    renameFile,
    newName,
    setNewName,
    handleRename,
    openRenameDialog,
  };
}
