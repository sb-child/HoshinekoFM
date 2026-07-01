import React, { createContext, useContext, useState, useCallback } from 'react';
import type { ReactNode } from 'react';
import type { IFile } from '../types/files';

interface DragState {
  files: IFile[];
  sourcePath: string;
}

interface DragContextType {
  dragState: DragState | null;
  startDrag: (files: IFile[], sourcePath: string) => void;
  endDrag: () => void;
}

const DragContext = createContext<DragContextType | undefined>(undefined);

export const DragProvider: React.FC<{ children: ReactNode }> = ({ children }) => {
  const [dragState, setDragState] = useState<DragState | null>(null);

  const startDrag = useCallback((files: IFile[], sourcePath: string) => {
    setDragState({ files, sourcePath });
  }, []);

  const endDrag = useCallback(() => {
    setDragState(null);
  }, []);

  return (
    <DragContext.Provider value={{ dragState, startDrag, endDrag }}>
      {children}
    </DragContext.Provider>
  );
};

export const useDrag = () => {
  const context = useContext(DragContext);
  if (!context) {
    throw new Error('useDrag must be used within a DragProvider');
  }
  return context;
};
