import { useState, useCallback } from 'react';

export interface CreateDialogState {
  type: "file" | "folder";
  defaultName: string;
  existingNames: string[];
  resolve: (name: string | null) => void;
}

export function useCreateDialog() {
  const [createDialog, setCreateDialog] = useState<CreateDialogState | null>(null);

  const handleCreateDialog = useCallback(
    (type: "file" | "folder", defaultName: string, existingNames: string[]) => {
      return new Promise<string | null>((resolve) => {
        setCreateDialog({ type, defaultName, existingNames, resolve });
      });
    },
    [],
  );

  return { createDialog, setCreateDialog, handleCreateDialog };
}
