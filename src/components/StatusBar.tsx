import React from 'react';
import type { IFile } from '../types/files';
import { getFileTypeDescription } from '../utils/mimeTypes';
import { t } from '../i18n';

interface StatusBarProps {
    totalItems: number;
    selectedCount: number;
    selectionHint?: string | null;
    hoveredFile?: IFile | null;
}

export const StatusBar: React.FC<StatusBarProps> = ({ totalItems, selectedCount, selectionHint, hoveredFile }) => {
  const fileType = hoveredFile ? getFileTypeDescription(hoveredFile) : null;

  return (
    <div style={{
      height: '24px',
      borderTop: '1px solid var(--border-color)',
      background: 'var(--surface-color)',
      color: 'var(--text-secondary)',
      fontSize: '12px',
      display: 'flex',
      alignItems: 'center',
      padding: '0 16px',
      gap: '16px',
      flexShrink: 0
    }}>
      <span style={{ flexShrink: 0 }}>{t("status.items", totalItems)}</span>
      {selectedCount > 0 && (
        <span style={{ flexShrink: 0 }}>{t("status.selected", selectedCount)}</span>
      )}
      {selectionHint && (
        <span style={{ flexShrink: 0 }}>{selectionHint}</span>
      )}
      {hoveredFile && (
        <span style={{
          marginLeft: 'auto',
          color: 'var(--text-primary)',
          display: 'flex',
          minWidth: 0,
          gap: '4px',
          textAlign: 'right',
        }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {hoveredFile.name}
          </span>
          <span style={{ flexShrink: 0 }}>({fileType})</span>
        </span>
      )}
    </div>
  );
};
