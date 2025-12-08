import React, { useState, useEffect, useMemo } from 'react';
import { Dialog } from './Dialog';
import { Button } from './Button';
import { Icon } from './Icon';

interface OpenWithDialogProps {
    open: boolean;
    onClose: () => void;
    onSelect: (exec: string) => void;
}

interface AppEntry {
    name: string;
    icon: string;
    exec: string;
}

export const OpenWithDialog: React.FC<OpenWithDialogProps> = ({ open, onClose, onSelect }) => {
    const [apps, setApps] = useState<AppEntry[]>([]);
    const [search, setSearch] = useState('');
    const [selectedApp, setSelectedApp] = useState<AppEntry | null>(null);

    useEffect(() => {
        if (open) {
            window.electron.getApps().then(setApps);
        }
    }, [open]);

    const filteredApps = useMemo(() => {
        return apps.filter(app => app.name.toLowerCase().includes(search.toLowerCase()));
    }, [apps, search]);

    const handleConfirm = () => {
        if (selectedApp) {
            onSelect(selectedApp.exec);
        }
    };

    return (
        <Dialog
            title="Open With..."
            open={open}
            onClose={onClose}
            actions={
                <>
                    <Button onClick={onClose} variant="text">Cancel</Button>
                    <Button onClick={handleConfirm} variant="filled" disabled={!selectedApp}>Open</Button>
                </>
            }
        >
            <div style={{ display: 'flex', flexDirection: 'column', gap: '16px', height: '400px', width: '400px' }}>
                <input
                    type="text"
                    placeholder="Search applications..."
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    className="md3-text-field"
                    style={{
                        padding: '12px',
                        borderRadius: '4px',
                        border: '1px solid var(--md-sys-color-outline)',
                        background: 'var(--md-sys-color-surface)',
                        color: 'var(--md-sys-color-on-surface)'
                    }}
                    autoFocus
                />

                <div style={{ flex: 1, overflowY: 'auto', display: 'flex', flexDirection: 'column', gap: '4px' }}>
                    {filteredApps.map((app, idx) => (
                        <div
                            key={idx}
                            onClick={() => setSelectedApp(app)}
                            style={{
                                display: 'flex', alignItems: 'center', gap: '12px', padding: '8px 12px',
                                borderRadius: '8px',
                                cursor: 'pointer',
                                background: selectedApp === app ? 'var(--md-sys-color-secondary-container)' : 'transparent',
                                color: selectedApp === app ? 'var(--md-sys-color-on-secondary-container)' : 'var(--md-sys-color-on-surface)'
                            }}
                        >
                            {/* Icon Placeholder or generic */}
                            <div style={{ width: '32px', height: '32px', display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(128,128,128,0.2)', borderRadius: '4px' }}>
                                {/* Attempt to render icon if simple path or generic. Most linux icons are names needing lookup. */}
                                <Icon name="apps" style={{ fontSize: '20px' }} />
                            </div>
                            <div style={{ fontWeight: 500 }}>{app.name}</div>
                        </div>
                    ))}
                </div>
            </div>
        </Dialog>
    );
};
