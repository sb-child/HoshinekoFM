import React, { useEffect, useState } from 'react';
import { Icon } from './Icon';
import './Dashboard.css';
import { useLocalStorage } from '../hooks/useLocalStorage';
import type { IFile } from '../types/files';

interface DashboardProps {
    onNavigate: (path: string) => void;
}

interface StorageStats {
    total: number;
    used: number;
    free: number;
}

interface PinnedItem {
    name: string;
    path: string;
    icon?: string;
}

export const Dashboard: React.FC<DashboardProps> = ({ onNavigate }) => {
    const [greeting, setGreeting] = useState('');
    const [storage, setStorage] = useState<StorageStats | null>(null);

    // Persisted State
    const [pinnedItems, setPinnedItems] = useLocalStorage<PinnedItem[]>('dashboard.pinned', [
        { name: 'Home', path: '/home/bhimio' },
        { name: 'Downloads', path: '/home/bhimio/Downloads' },
        { name: 'Documents', path: '/home/bhimio/Documents' }
    ]);

    // Recent files
    const [recentFiles] = useLocalStorage<IFile[]>('dashboard.recent', []);

    useEffect(() => {
        const hour = new Date().getHours();
        if (hour < 12) setGreeting('Good Morning');
        else if (hour < 18) setGreeting('Good Afternoon');
        else setGreeting('Good Evening');

        // Fetch storage
        if (window.electron) {
            window.electron.getStorageUsage().then(stats => {
                if (stats) setStorage(stats);
            });
        }
    }, []);

    const handleAddPin = async () => {
        if (window.electron) {
            const path = await window.electron.openFileDialog();
            if (path) {
                const name = path.split('/').pop() || path;
                setPinnedItems(prev => [...prev, { name, path }]);
            }
        }
    };

    const handleRemovePin = (e: React.MouseEvent, index: number) => {
        e.stopPropagation();
        setPinnedItems(prev => prev.filter((_, i) => i !== index));
    };

    const formatBytes = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const n = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, n)).toFixed(1)) + ' ' + ['B', 'KB', 'MB', 'GB', 'TB'][n];
    };

    const getUsagePercent = () => {
        if (!storage) return 0;
        return (storage.used / storage.total) * 100;
    };

    return (
        <div className="dashboard-container fade-in">
            <header className="dashboard-header">
                <h1 className="greeting">{greeting}</h1>
                <p className="subtitle">Welcome back to your command center.</p>
            </header>

            <div className="dashboard-grid">
                {/* Storage Widget */}
                <div className="dashboard-card storage-card">
                    <div className="card-header">
                        <Icon name="hard_drive" filled />
                        <span>System Storage</span>
                    </div>
                    {storage ? (
                        <div className="storage-info">
                            <div className="usage-bar">
                                <div className="usage-fill" style={{ width: `${getUsagePercent()}%` }}></div>
                            </div>
                            <div className="storage-text">
                                <span>{formatBytes(storage.used)} used</span>
                                <span>{formatBytes(storage.total)} total</span>
                            </div>
                        </div>
                    ) : (
                        <div className="storage-loading">Loading stats...</div>
                    )}
                </div>

                {/* Pinned Folders */}
                <div className="dashboard-card pinned-card">
                    <div className="card-header">
                        <Icon name="push_pin" filled />
                        <span>Pinned</span>
                    </div>
                    <div className="pinned-grid">
                        {pinnedItems.map((item, idx) => (
                            <div key={idx} className="pinned-item" onClick={() => onNavigate(item.path)}>
                                <div className="pinned-icon">
                                    <Icon name={item.name === 'Home' ? 'home' : 'folder'} size={32} />
                                </div>
                                <span>{item.name}</span>
                                <div className="pin-remove" onClick={(e) => handleRemovePin(e, idx)} title="Unpin">
                                    <Icon name="close" size={14} />
                                </div>
                            </div>
                        ))}
                        <div className="pinned-item add-pin" onClick={handleAddPin}>
                            <div className="pinned-icon">
                                <Icon name="add" />
                            </div>
                            <span>Add</span>
                        </div>
                    </div>
                </div>

                {/* Recent Files */}
                <div className="dashboard-card recent-card">
                    <div className="card-header">
                        <Icon name="history" filled />
                        <span>Recent</span>
                    </div>
                    <div className="recent-list">
                        {recentFiles.length === 0 ? (
                            <div className="recent-placeholder">No recent files yet.</div>
                        ) : (
                            recentFiles.slice(0, 10).map((file, idx) => (
                                <div key={idx} className="recent-item" onClick={() => onNavigate(file.path)}>
                                    <Icon name={file.isDirectory ? 'folder' : 'article'} size={20} />
                                    <span className="recent-name">{file.name}</span>
                                    <span className="recent-path">{file.path}</span>
                                </div>
                            ))
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
};
