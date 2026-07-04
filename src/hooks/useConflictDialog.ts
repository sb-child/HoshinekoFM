import { useState, useCallback } from 'react';
import type { ConflictEntry, ConflictResult } from '../utils/fileConflict';

interface SingleConflictState {
  conflict: ConflictEntry;
  existingNames: string[];
  destDir: string;
  sourcePath?: string;
  operation?: "move" | "copy";
  resolve: (result: ConflictResult) => void;
}

interface MultiConflictState {
  conflicts: ConflictEntry[];
  destDir: string;
  existingNames: string[];
  resolve: (result: ConflictResult) => void;
  sourcePath?: string;
  operation?: "move" | "copy";
}

export function useConflictDialog() {
  const [singleConflict, setSingleConflict] = useState<SingleConflictState | null>(null);
  const [multiConflict, setMultiConflict] = useState<MultiConflictState | null>(null);

  const handleConflictDialog = useCallback(
    (conflicts: ConflictEntry[], destDir: string, existingNames: string[], sourcePath?: string, operation?: "move" | "copy") => {
      return new Promise<ConflictResult>((resolve) => {
        if (conflicts.length === 1) {
          setSingleConflict({ conflict: conflicts[0], existingNames, destDir, sourcePath, operation, resolve });
        } else {
          setMultiConflict({ conflicts, destDir, existingNames, resolve, sourcePath, operation });
        }
      });
    },
    [],
  );

  return { singleConflict, setSingleConflict, multiConflict, setMultiConflict, handleConflictDialog };
}
