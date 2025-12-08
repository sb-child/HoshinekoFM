import React, { useState, useEffect, useCallback, useMemo } from 'react';
import { useToast } from '../contexts/ToastContext';
import { useClipboard } from '../contexts/ClipboardContext';
import { StatusBar } from './StatusBar';
import { FileList } from './FileList';
import { FileListSkeleton } from './FileListSkeleton';
import { IconButton } from './IconButton';
import { Icon } from './Icon';
import { FileSystemService } from '../services/FileSystemService';
import type { IFile } from '../types/files';

import { Omnibar } from './Omnibar';
import { Dashboard } from './Dashboard';
import { useLocalStorage } from '../hooks/useLocalStorage';
import { getSemanticGroup, GROUP_ORDER } from '../utils/fileUtils';

interface ExplorerTabProps {
    tabId: string;
    isActive: boolean;
    initialPath: string;
    onPathChange: (id: string, path: string) => void;
    onContextMenu: (e: React.MouseEvent, file: IFile | null) => void;
    showHiddenFiles: boolean;
    iconSize: number;
    viewMode: 'grid' | 'list';
    filledIcons: boolean;
}

export function ExplorerTab({ tabId, isActive, initialPath, onPathChange, onContextMenu, showHiddenFiles, iconSize, viewMode, filledIcons }: ExplorerTabProps) {
    const [currentPath, setCurrentPath] = useState(initialPath);
    const [files, setFiles] = useState<IFile[]>([]);
    const [loading, setLoading] = useState(false);
    const { showToast } = useToast();
    const { copy, cut, clipboard, clear: clearClipboard } = useClipboard();

    // Track recents
    const [, setRecentFiles] = useLocalStorage<IFile[]>('dashboard.recent', []);

    const addToRecents = useCallback((path: string) => {
        if (path === 'app://dashboard') return;

        const name = path.split('/').pop() || path;
        const newItem: IFile = {
            name,
            path,
            isDirectory: true, // Assumed if navigating to it
            size: 0,
            mtime: new Date()
        };

        setRecentFiles(prev => {
            // Remove duplicates or move to top
            const filtered = prev.filter(f => f.path !== path);
            return [newItem, ...filtered].slice(0, 20); // Keep last 20
        });
    }, [setRecentFiles]);

    // Search State
    const [searchActive, setSearchActive] = useState(false);
    const [searchQuery, setSearchQuery] = useState('');

    const handleSearch = async (query: string) => {
        setLoading(true);
        setSearchActive(true);
        setSearchQuery(query);
        try {
            if (window.electron && window.electron.search) {
                const results = await window.electron.search(currentPath, query);
                setFiles(results);
            }
        } catch (e) {
            console.error(e);
            showToast('Search failed', 'error');
        }
        setLoading(false);
    };

    const loadPath = useCallback(async (path: string) => {
        setSearchActive(false); // Reset search
        setSearchQuery('');

        if (path === 'app://dashboard') {
            setCurrentPath(path);
            onPathChange(tabId, path);
            return;
        }

        setLoading(true);
        try {
            const items = await FileSystemService.listDir(path);
            setFiles(items);
            setCurrentPath(path);
            onPathChange(tabId, path);
            addToRecents(path); // Track it
        } catch (e) {
            console.error('Failed to load path', path, e);
        }
        setLoading(false);
    }, [onPathChange, tabId, addToRecents]);

    useEffect(() => {
        if (initialPath) {
            loadPath(initialPath);
        }
    }, [initialPath, loadPath]); // Added loadPath dependency



    // Actually, we want to control path internal to tab mostly.

    const handleNavigate = (file: IFile) => {
        if (file.isDirectory) {
            loadPath(file.path);
        } else {
            // Open file
            FileSystemService.open(file.path);
        }
    };

    const handleUp = async () => {
        if (window.electron && currentPath) {
            const parent = await window.electron.getParentPath(currentPath);
            loadPath(parent);
        }
    };

    // Sort State
    const [sortBy, setSortBy] = useState<'name' | 'size' | 'date'>('name');
    const [sortOrder, setSortOrder] = useState<'asc' | 'desc'>('asc');

    // Grouping State
    const [groupingEnabled, setGroupingEnabled] = useState(true);

    // Filter AND Sort files
    const sortedFiles = useMemo(() => {
        let filtered = files.filter(f => showHiddenFiles || !f.name.startsWith('.'));
        return filtered.sort((a: IFile, b: IFile) => {
            // Always folders first? 
            // If grouping enabled, use group order.
            // Note: getSemanticGroup returns 'Folders' for directories.

            if (groupingEnabled) {
                const groupA = getSemanticGroup(a);
                const groupB = getSemanticGroup(b);
                if (groupA !== groupB) {
                    return GROUP_ORDER.indexOf(groupA) - GROUP_ORDER.indexOf(groupB);
                }
            } else {
                // Standard Folders First if not grouping (or implicit in grouping)
                if (a.isDirectory !== b.isDirectory) {
                    return a.isDirectory ? -1 : 1;
                }
            }

            let result = 0;
            switch (sortBy) {
                case 'name':
                    result = a.name.localeCompare(b.name);
                    break;
                case 'size':
                    result = a.size - b.size;
                    break;
                case 'date':
                    result = a.mtime.getTime() - b.mtime.getTime();
                    break;
            }
            return sortOrder === 'asc' ? result : -result;
        });
    }, [files, showHiddenFiles, sortBy, sortOrder, groupingEnabled]);


    // Selection State
    const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
    const [lastSelectedPath, setLastSelectedPath] = useState<string | null>(null);

    // Clear selection on path change
    useEffect(() => {
        setSelectedFiles(new Set());
        setLastSelectedPath(null);
    }, [currentPath]);

    const handleSelect = (file: IFile, toggle: boolean, range: boolean) => {
        const newSelection = new Set(toggle ? selectedFiles : []);

        if (range && lastSelectedPath) {
            // Find range using SORTED files
            const start = sortedFiles.findIndex(f => f.path === lastSelectedPath);
            const end = sortedFiles.findIndex(f => f.path === file.path);
            if (start !== -1 && end !== -1) {
                const low = Math.min(start, end);
                const high = Math.max(start, end);
                for (let i = low; i <= high; i++) {
                    newSelection.add(sortedFiles[i].path);
                }
            } else {
                newSelection.add(file.path);
            }
        } else if (toggle) {
            if (selectedFiles.has(file.path)) {
                newSelection.delete(file.path);
            } else {
                newSelection.add(file.path);
                setLastSelectedPath(file.path);
            }
        } else {
            // Single select
            newSelection.add(file.path);
            setLastSelectedPath(file.path);
        }

        setSelectedFiles(newSelection);
    };

    // Expose method to force reload or navigate from outside?
    // For now simple.

    // Keyboard Shortcuts
    useEffect(() => {
        if (!isActive) return;

        const handleKeyDown = async (e: KeyboardEvent) => {
            // Select All (Ctrl+A)
            if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
                e.preventDefault();
                const allPaths = new Set(sortedFiles.map(f => f.path));
                setSelectedFiles(allPaths);
                return;
            }

            // Refresh (F5)
            if (e.key === 'F5') {
                e.preventDefault();
                loadPath(currentPath);
                return;
            }

            // Delete (Del)
            if (e.key === 'Delete') {
                e.preventDefault();
                if (selectedFiles.size > 0) {
                    // Logic to delete files
                    // We need a confirm dialog really, but for now simple confirm
                    if (window.confirm(`Delete ${selectedFiles.size} items?`)) {
                        for (const path of selectedFiles) {
                            await FileSystemService.trash(path);
                        }
                        showToast(`Deleted ${selectedFiles.size} items`, 'success');
                        loadPath(currentPath); // Refresh
                    }
                }
                return;
            }

            // Copy (Ctrl+C)
            if ((e.ctrlKey || e.metaKey) && e.key === 'c') {
                if (selectedFiles.size > 0) {
                    const filesToCopy = sortedFiles.filter(f => selectedFiles.has(f.path));
                    copy(filesToCopy);
                    showToast(`Copied ${selectedFiles.size} items`, 'info');
                }
                return;
            }

            // Cut (Ctrl+X)
            if ((e.ctrlKey || e.metaKey) && e.key === 'x') {
                if (selectedFiles.size > 0) {
                    const filesToCut = sortedFiles.filter(f => selectedFiles.has(f.path));
                    cut(filesToCut);
                    showToast(`Cut ${selectedFiles.size} items`, 'info');
                }
                return;
            }

            // Paste (Ctrl+V)
            if ((e.ctrlKey || e.metaKey) && e.key === 'v') {
                if (clipboard && clipboard.files.length > 0) {
                    let count = 0;
                    for (const file of clipboard.files) {
                        const destPath = currentPath + '/' + file.name;
                        try {
                            if (clipboard.operation === 'copy') {
                                await FileSystemService.copy(file.path, destPath);
                            } else {
                                await FileSystemService.move(file.path, destPath);
                            }
                            count++;
                        } catch (err) {
                            console.error("Paste error", err);
                        }
                    }

                    if (clipboard.operation === 'cut') {
                        clearClipboard();
                    }

                    if (count > 0) {
                        showToast(`Pasted ${count} items`, 'success');
                        loadPath(currentPath); // Refresh
                    }
                }
                return;
            }
        };

        window.addEventListener('keydown', handleKeyDown);
        return () => window.removeEventListener('keydown', handleKeyDown);
    }, [isActive, sortedFiles, selectedFiles, currentPath, loadPath, showToast]);

    return (
        <div style={{ display: isActive ? 'flex' : 'none', flexDirection: 'column', flex: 1, height: '100%', overflow: 'hidden' }}>
            {/* Top Bar (Breadcrumb/Path) */}
            {(currentPath !== 'app://dashboard') && (
                <div style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '16px', padding: '8px 24px 0' }}>
                    <IconButton onClick={handleUp} variant="standard">
                        <Icon name="arrow_upward" />
                    </IconButton>
                    <div style={{ flex: 1, overflow: 'hidden' }}>
                        <Omnibar
                            currentPath={currentPath}
                            onNavigate={loadPath}
                            onSearch={handleSearch}
                        />
                    </div>
                    {/* Sort Controls */}
                    <div style={{ display: 'flex', gap: '4px' }}>
                        <IconButton
                            variant={groupingEnabled ? 'filled' : 'standard'}
                            onClick={() => setGroupingEnabled(!groupingEnabled)}
                            title="Toggle Grouping"
                        >
                            <Icon name="view_agenda" />
                        </IconButton>
                        <div style={{ width: '1px', background: 'var(--md-sys-color-outline-variant)', margin: '0 4px' }} />
                        <IconButton
                            variant={sortBy === 'name' ? 'filled' : 'standard'}
                            onClick={() => {
                                if (sortBy === 'name') setSortOrder(prev => prev === 'asc' ? 'desc' : 'asc');
                                else { setSortBy('name'); setSortOrder('asc'); }
                            }}
                            title="Sort by Name"
                        >
                            <Icon name="sort_by_alpha" />
                        </IconButton>
                        <IconButton
                            variant={sortBy === 'date' ? 'filled' : 'standard'}
                            onClick={() => {
                                if (sortBy === 'date') setSortOrder(prev => prev === 'asc' ? 'desc' : 'asc');
                                else { setSortBy('date'); setSortOrder('desc'); }
                            }}
                            title="Sort by Date"
                        >
                            <Icon name="calendar_today" />
                        </IconButton>
                    </div>
                </div>
            )}

            {currentPath === 'app://dashboard' ? (
                <div style={{ display: 'flex', flexDirection: 'column', flex: 1, height: '100%', overflow: 'hidden' }}>
                    <Dashboard onNavigate={loadPath} />
                </div>
            ) : loading ? (
                <FileListSkeleton viewMode={viewMode} iconSize={iconSize} />
            ) : (
                <div style={{ display: 'flex', flexDirection: 'column', flex: 1, overflow: 'hidden' }}>
                    {searchActive && (
                        <div style={{ padding: '8px 24px', background: 'var(--md-sys-color-surface-container)', color: 'var(--md-sys-color-on-surface-variant)', fontSize: '14px', display: 'flex', alignItems: 'center', gap: '8px' }}>
                            <Icon name="search" />
                            <span>Found {files.length} results for "{searchQuery}"</span>
                            <IconButton onClick={() => loadPath(currentPath)} variant="standard" title="Clear Search">
                                <Icon name="close" />
                            </IconButton>
                        </div>
                    )}
// ...
                    <div
                        style={{ flex: 1, overflow: 'auto' }}
                        onClick={(e) => {
                            // Only if clicking the background itself, not bubbling from a file item (which handles its own events)
                            // Actually file items stop propagation?
                            // If I click blank space, e.target will be this div or a spacer.
                            if (e.target === e.currentTarget) {
                                setSelectedFiles(new Set());
                            }
                        }}
                        onDragOver={(e) => {
                            e.preventDefault();
                            e.dataTransfer.dropEffect = 'copy';
                        }}
                        onDrop={async (e) => {
                            e.preventDefault();
                            // Drop logic ...
                            // (Kept as is, but simpler to just re-render the wrapper div if I want to wrap content)
                            // Actually, I can just wrap the content div.
                            const droppedFiles = Array.from(e.dataTransfer.files);
                            if (droppedFiles.length > 0 && currentPath) {
                                let count = 0;
                                for (const file of droppedFiles) {
                                    // @ts-ignore
                                    const sourcePath = file.path;
                                    if (sourcePath) {
                                        const fileName = sourcePath.split(/[/\\\\]/).pop();
                                        const destPath = `${currentPath}/${fileName}`.replace(/\\+/g, '/');
                                        if (window.electron && window.electron.copyFile) {
                                            await window.electron.copyFile(sourcePath, destPath);
                                            count++;
                                        }
                                    }
                                }
                                if (count > 0) {
                                    showToast(`Imported ${count} files`, 'success');
                                    loadPath(currentPath);
                                }
                            }
                        }}
                    >
                        <FileList
                            files={sortedFiles}
                            selectedFiles={selectedFiles}
                            onSelect={handleSelect}
                            onNavigate={handleNavigate}
                            onContextMenu={(e, file) => {
                                if (file && !selectedFiles.has(file.path)) {
                                    handleSelect(file, false, false);
                                }
                                onContextMenu(e, file);
                            }}
                            iconSize={iconSize}
                            viewMode={viewMode}
                            filledIcons={filledIcons}
                            groupingEnabled={groupingEnabled}
                        />
                    </div>
                </div>
            )}

            {currentPath !== 'app://dashboard' && (
                <StatusBar totalItems={files.length} selectedCount={selectedFiles.size} />
            )}
        </div>
    );
};
