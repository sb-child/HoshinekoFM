import React, { useState, useEffect, useMemo } from 'react';
import { Dialog } from './Dialog';
import { Button } from './Button';
import { Icon } from './Icon';
import { OutlinedTextField } from './md';
import { showToast } from '../utils/toast';
import { formatFileOpError } from '../utils/fileOperations';
import { t as ti } from '../i18n';

interface OpenWithDialogProps {
    open: boolean;
    onClose: () => void;
    onSelect: (exec: string, desktopFile?: string) => void;
}

interface AppEntry {
    name: string;
    icon: string | null;
    exec: string;
    desktopFile?: string;
}

const labelToKey: Record<string, string> = {
  'Open With...': 'open_with.title',
  'Cancel': 'dialog.button.cancel',
  'Open': 'dialog.button.open',
  'Search applications...': 'open_with.search',
  'Recommended': 'open_with.recommended',
  'All Applications': 'open_with.all'
};

const tOpenWith = (text: string) => {
  const key = labelToKey[text];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return key ? (ti as any)(key) : text;
};

export const OpenWithDialog: React.FC<OpenWithDialogProps & { path: string }> = ({ open, onClose, onSelect, path }) => {
  const [allApps, setAllApps] = useState<AppEntry[]>([]);
  const [recommendedApps, setRecommendedApps] = useState<AppEntry[]>([]);
  const [search, setSearch] = useState('');
  const [selectedApp, setSelectedApp] = useState<AppEntry | null>(null);

  useEffect(() => {
    if (open) {
      window.electron.getApps().then(setAllApps);
      if (path) {
        window.electron.getRecommendedApps(path).then(apps =>
          setRecommendedApps(apps.map(a => ({ name: a.name, icon: a.icon, exec: a.exec, desktopFile: a.path })))
        );
      } else {
        setRecommendedApps([]); // eslint-disable-line react-hooks/set-state-in-effect
      }
    }
  }, [open, path]);

  const filteredAllApps = useMemo(() => {
    return allApps.filter(app => app.name.toLowerCase().includes(search.toLowerCase()));
  }, [allApps, search]);

  // 核心修复：加入容错捕获，防止后端 spawn 找不到执行文件时主进程抛错崩溃
  const handleConfirm = async () => {
    if (selectedApp) {
      try {
        // 执行打开操作
        await onSelect(selectedApp.exec, selectedApp.desktopFile);
        onClose();
      } catch (error) {
        console.error(ti('toast.launch_failed', selectedApp.exec, String(error)));
        showToast(formatFileOpError('启动程序', selectedApp.name, error), 'error');
      }
    }
  };

  const renderAppItem = (app: AppEntry, idx: number) => (
    <div
      key={`${app.name}-${idx}`}
      onClick={() => setSelectedApp(app)}
      style={{
        display: 'flex', alignItems: 'center', gap: '12px', padding: '8px 12px',
        borderRadius: '8px',
        cursor: 'pointer',
        background: selectedApp === app ? 'var(--md-sys-color-secondary-container)' : 'transparent',
        color: selectedApp === app ? 'var(--md-sys-color-on-secondary-container)' : 'var(--md-sys-color-on-surface)'
      }}
    >
      <div style={{ width: '32px', height: '32px', display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(128,128,128,0.2)', borderRadius: '4px' }}>
        <Icon name="apps" style={{ fontSize: '20px' }} />
      </div>
      <div style={{ fontWeight: 500 }}>{app.name}</div>
    </div>
  );

  return (
    <Dialog
      title={tOpenWith('Open With...')}
      open={open}
      onClose={onClose}
      actions={
        <>
          <Button onClick={onClose} variant="text">{tOpenWith('Cancel')}</Button>
          <Button onClick={handleConfirm} variant="filled" disabled={!selectedApp}>{tOpenWith('Open')}</Button>
        </>
      }
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: '16px', height: '500px', width: '400px' }}>
        <OutlinedTextField
          label={tOpenWith('Search applications...')}
          value={search}
          onInput={(e) => setSearch((e.target as HTMLInputElement).value)}
          style={{ width: '100%' }}
        />

        <div style={{ flex: 1, overflowY: 'auto', display: 'flex', flexDirection: 'column', gap: '4px' }}>
          {recommendedApps.length > 0 && !search && (
            <>
              <div style={{ fontSize: '12px', fontWeight: 600, color: 'var(--md-sys-color-primary)', marginTop: '8px', paddingLeft: '12px' }}>
                {tOpenWith('Recommended')}
              </div>
              {recommendedApps.map((app, idx) => renderAppItem(app, idx))}
              <div style={{ height: '1px', background: 'var(--md-sys-color-outline-variant)', margin: '8px 0' }} />
              <div style={{ fontSize: '12px', fontWeight: 600, color: 'var(--md-sys-color-primary)', paddingLeft: '12px' }}>
                {tOpenWith('All Applications')}
              </div>
            </>
          )}
          {filteredAllApps.map((app, idx) => renderAppItem(app, idx))}
        </div>
      </div>
    </Dialog>
  );
};
