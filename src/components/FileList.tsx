import React from 'react';
import type { IFile } from '../types/files';
import { Icon } from './Icon';
import './FileList.css';
import { getSemanticGroup } from '../utils/fileUtils';

interface FileListProps {
    files: IFile[];
    selectedFiles: Set<string>;
    onSelect: (file: IFile, toggle: boolean, range: boolean) => void;
    onNavigate: (file: IFile) => void;
    onContextMenu?: (e: React.MouseEvent, file: IFile) => void;
    viewMode: 'grid' | 'list';
    iconSize: number;
    filledIcons: boolean;
    groupingEnabled?: boolean;
}

export const FileList: React.FC<FileListProps> = ({ files, selectedFiles, onSelect, onNavigate, onContextMenu, viewMode, iconSize, filledIcons, groupingEnabled }) => {
    // Files are likely already sorted by group if groupingEnabled is true
    const sortedFiles = files;
    let lastGroup = '';

    return (
        <div
            className={`file-list ${viewMode}`}
            style={{
                // Dynamically update grid columns based on iconSize + padding/gap
                gridTemplateColumns: viewMode === 'grid' ? `repeat(auto-fill, minmax(${iconSize + 32}px, 1fr))` : '1fr'
            }}
        >
            {sortedFiles.map((file, index) => {
                const currentGroup = groupingEnabled ? getSemanticGroup(file) : '';
                const showHeader = groupingEnabled && currentGroup !== lastGroup;
                if (showHeader) lastGroup = currentGroup;

                return (
                    <React.Fragment key={file.path}>
                        {showHeader && (
                            <div
                                className="file-group-header"
                                style={{
                                    gridColumn: '1 / -1',
                                    padding: '16px 8px 8px',
                                    fontWeight: 500,
                                    color: 'var(--md-sys-color-primary)',
                                    borderBottom: '1px solid var(--md-sys-color-outline-variant)',
                                    marginBottom: '8px',
                                    marginTop: index > 0 ? '16px' : '0'
                                }}
                            >
                                {currentGroup}
                            </div>
                        )}
                        <div
                            className={`file-list-item ${selectedFiles.has(file.path) ? 'selected' : ''}`}
                            onClick={(e) => {
                                // Click logic: 
                                // 1. If modifier keys (Ctrl/Shift/Meta) -> standard selection logic
                                // 2. If NOT modifier keys AND already selected -> Open (User Request)
                                // 3. Otherwise -> Select
                                const isModifier = e.ctrlKey || e.metaKey || e.shiftKey;
                                const isSelected = selectedFiles.has(file.path);

                                if (!isModifier && isSelected) {
                                    onNavigate(file);
                                } else {
                                    onSelect(file, e.ctrlKey || e.metaKey, e.shiftKey);
                                }
                            }}
                            onDoubleClick={() => onNavigate(file)}
                            onContextMenu={(e) => onContextMenu && onContextMenu(e, file)}
                            draggable={true}
                            onDragStart={(e) => {
                                e.preventDefault();
                                if (window.electron && window.electron.startDrag) {
                                    window.electron.startDrag(file.path, file.path);
                                }
                            }}
                            tabIndex={0}
                            role="button"
                            style={viewMode === 'list' ? {
                                gridTemplateColumns: `${iconSize + 16}px 1fr 100px`,
                                height: `${Math.max(52, iconSize + 16)}px`
                            } : undefined}
                        >
                            <span className="file-icon" style={{ width: `${iconSize}px`, height: `${iconSize}px`, fontSize: `${iconSize}px` }}>
                                {isImage(file.name) ? (
                                    <img
                                        src={`media://${file.path}`}
                                        alt={file.name}
                                        className="file-thumbnail"
                                        onError={(e) => {
                                            (e.currentTarget).style.display = 'none';
                                        }}
                                        style={{
                                            width: viewMode === 'list' ? `${iconSize}px` : '100%',
                                            height: viewMode === 'list' ? `${iconSize}px` : '100%'
                                        }}
                                    />
                                ) : null}


                                {!isImage(file.name) && (
                                    <Icon
                                        name={file.isDirectory ? 'folder' : getFileIcon(file.name)}
                                        filled={filledIcons}
                                        className={file.isDirectory ? 'folder-icon' : 'doc-icon'}
                                        style={{ fontSize: `${iconSize}px` }}
                                    />
                                )}
                            </span>
                            <span className="file-name">{file.name}</span>
                            <span className="file-size">{file.isDirectory ? '' : formatSize(file.size)}</span>
                        </div>
                    </React.Fragment>
                );
            })}
        </div>
    );
};

function getFileIcon(filename: string): string {
    const ext = filename.split('.').pop()?.toLowerCase();
    switch (ext) {
        case 'png': case 'jpg': case 'jpeg': case 'gif': return 'image';
        case 'mp3': case 'wav': return 'audio_file';
        case 'mp4': case 'mkv': return 'movie';
        case 'pdf': return 'picture_as_pdf';
        case 'txt': case 'md': return 'article';
        case 'zip': case 'tar': case 'gz': return 'folder_zip';
        default: return 'insert_drive_file';
    }
}

function isImage(filename: string): boolean {
    const ext = filename.split('.').pop()?.toLowerCase();
    return ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp'].includes(ext || '');
}

function formatSize(bytes: number) {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}
