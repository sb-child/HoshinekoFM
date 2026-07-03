import React, { useState, useEffect, useMemo, useRef } from 'react';
import { Dialog } from './Dialog';
import { Button } from './Button';
import { OutlinedTextField } from './md';
import { generateSafeName, splitNameExt, truncateDirPath } from '../utils/fileConflict';
import { t } from '../i18n';
import './NameInputDialog.css';

interface NameInputDialogProps {
  title: string;
  defaultName: string;
  isDir: boolean;
  parentDir?: string;
  existingNames: string[];
  sourcePath?: string;
  operation?: "move" | "copy";
  destDir?: string;
  onConfirm: (name: string) => void;
  onCancel: () => void;
}

export const NameInputDialog: React.FC<NameInputDialogProps> = ({
  title,
  defaultName,
  isDir,
  existingNames,
  sourcePath,
  operation,
  destDir,
  onConfirm,
  onCancel,
}) => {
  const existingSet = useMemo(() => new Set(existingNames), [existingNames]);

  const computeDefault = (): string => {
    const { base, ext } = splitNameExt(defaultName, isDir);
    if (existingSet.has(defaultName)) {
      return generateSafeName(base, ext, existingSet, isDir);
    }
    return defaultName;
  };

  const [value, setValue] = useState(() => computeDefault());
  const [conflict, setConflict] = useState(false);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const inputRef = useRef<any>(null);

  // Recompute default when opened with new props
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setValue(computeDefault());
     
    setConflict(false);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultName, isDir, existingSet]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleChange = (e: Event) => {
    const v = (e.target as HTMLInputElement).value;
    setValue(v);
    const trimmed = v.trim();
    if (!trimmed) {
      setConflict(false);
      return;
    }
    if (trimmed.includes('/') || trimmed.includes('..')) {
      setConflict(false);
    } else {
      const checkName = isDir ? trimmed.replace(/\/$/, '') : trimmed;
      setConflict(existingSet.has(checkName));
    }
  };

  const handleConfirm = () => {
    let name = value.trim();
    if (!name) return;
    if (isDir && !name.endsWith('/')) {
      name = name + '/';
    }
    const simple = !name.includes('/') && !name.includes('..');
    if (simple) {
      const checkName = name.endsWith('/') ? name.slice(0, -1) : name;
      if (existingSet.has(checkName)) {
        setConflict(true);
        return;
      }
    }
    setConflict(false);
    onConfirm(name);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleConfirm();
    }
  };

  const canConfirm = value.trim().length > 0 && !conflict;

  return (
    <Dialog
      title={title}
      open={true}
      onClose={onCancel}
      actions={
        <>
          <Button variant="text" onClick={onCancel}>
            {t('dialog.button.cancel')}
          </Button>
          <Button onClick={handleConfirm} disabled={!canConfirm}>
            {t('dialog.button.confirm')}
          </Button>
        </>
      }
    >
      <div className="name-input-container">
        {sourcePath && (
          <div className="conflict-info-section">
            <div className="conflict-info-row">
              <span className="conflict-info-label">{t('dialog.conflict.source_label')}</span>
              <span className="conflict-info-path" title={sourcePath}>
                {truncateDirPath(sourcePath, 48)}
              </span>
            </div>
            {operation && (
              <div className="conflict-info-row">
                <span className="conflict-info-label">{t('dialog.conflict.operation_label')}</span>
                <span className="conflict-info-value">
                  {operation === "copy" ? t('dialog.conflict.operation_copy') : t('dialog.conflict.operation_move')}
                </span>
              </div>
            )}
            {destDir && (
              <div className="conflict-info-row">
                <span className="conflict-info-label">{t('dialog.conflict.dest_label')}</span>
                <span className="conflict-info-path" title={destDir}>
                  {truncateDirPath(destDir, 48)}
                </span>
              </div>
            )}
          </div>
        )}
        <OutlinedTextField
          ref={inputRef}
          label={title}
          value={value}
          onInput={handleChange}
          onKeyDown={handleKeyDown}
          error={conflict}
          errorText={conflict ? t('error.name_exists', value) : ''}
          style={{ width: '100%' }}
        />
      </div>
    </Dialog>
  );
};
