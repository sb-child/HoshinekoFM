/* eslint-disable react-refresh/only-export-components */
import React, { createContext, useContext, useCallback, useRef } from 'react';
import type { ReactNode } from 'react';
import type { IFile } from '../types/files';

interface DragState {
  files: IFile[];
  sourcePath: string;
}

interface DragContextType {
  getDragState: () => DragState | null;
  startDrag: (files: IFile[], sourcePath: string) => void;
  endDrag: () => void;
}

const DragContext = createContext<DragContextType | undefined>(undefined);

export const DragProvider: React.FC<{ children: ReactNode }> = ({ children }) => {
  const dragRef = useRef<DragState | null>(null);

  const getDragState = useCallback(() => dragRef.current, []);

  const startDrag = useCallback((files: IFile[], sourcePath: string) => {
    dragRef.current = { files, sourcePath };
  }, []);

  const endDrag = useCallback(() => {
    dragRef.current = null;
  }, []);

  return (
    <DragContext.Provider value={{ getDragState, startDrag, endDrag }}>
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
