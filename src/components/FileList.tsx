import React, { useEffect, useState } from 'react';
import type { IFile } from '../types/files';
import { Icon } from './Icon';
import './FileList.css';
import { getSemanticGroup } from '../utils/fileUtils';

interface FileListProps {
    files: IFile[];
    selectedFiles: Set<string>;
    onSelect: (file: IFile, toggle: boolean, range: boolean) => void;
    onNavigate: (file: IFile) => void;
    // 单个文件的右键菜单事件
    onContextMenu?: (e: React.MouseEvent, file: IFile) => void;
    // 新增：空白处的右键菜单事件
    onBackgroundContextMenu?: (e: React.MouseEvent) => void;
    viewMode: 'grid' | 'list';
    iconSize: number;
    filledIcons: boolean;
    groupingEnabled?: boolean;
}

// 统一汉化词典，完美映射强类型分组与常见时间分组
const groupLocaleMap: Record<string, string> = {
    // 时间线分组
    'Today': '今天',
    'Yesterday': '昨天',
    'Earlier this week': '本周早些时候',
    'Earlier this month': '本月早些时候',
    'Earlier this year': '今年早些时候',
    'Older': '更早以前',
    
    // fileUtils.ts 对应的强类型标签汉化
    'Folders': '文件夹',
    'Files': '文件',
    'Media': '媒体文件',
    'Documents': '文档',
    'Code': '代码文件',
    'Archives': '压缩包',
    'Executables': '可执行文件',
    'Others': '其他文件'
};

const tGroup = (groupName: string): string => groupLocaleMap[groupName] || groupName;

export const FileList: React.FC<FileListProps> = ({ files, selectedFiles, onSelect, onNavigate, onContextMenu, onBackgroundContextMenu, viewMode, iconSize, filledIcons, groupingEnabled }) => {
    // Files are likely already sorted by group if groupingEnabled is true
    const sortedFiles = files;
    let lastGroup = '';

    // 用于记录加载失败的图片路径，以便降级渲染标准图标
    const [failedImages, setFailedImages] = useState<Set<string>>(new Set());

    return (
        <div
            className={`file-list ${viewMode}`}
            style={{
                // Dynamically update grid columns based on iconSize + padding/gap
                gridTemplateColumns: viewMode === 'grid' ? `repeat(auto-fill, minmax(${iconSize + 32}px, 1fr))` : '1fr',
                // 确保容器至少占满整个高度，使得在文件较少时点击下方空白处也能触发事件
                minHeight: '100%' 
            }}
            // 核心修改 1：捕获空白处的右键点击
            onContextMenu={(e) => {
                e.preventDefault();
                // 如果点击的地方不是文件项（冒泡上来的），则触发空白处菜单
                if (onBackgroundContextMenu) {
                    onBackgroundContextMenu(e);
                }
            }}
        >
            {sortedFiles.map((file, index) => {
                const currentGroup = groupingEnabled ? getSemanticGroup(file) : '';
                const showHeader = groupingEnabled && currentGroup !== lastGroup;
                if (showHeader) lastGroup = currentGroup;

                const isImg = isImage(file.name);
                const hasImageFailed = failedImages.has(file.path);

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
                                {/* 通过汉化词典输出无缝翻译的分组名称 */}
                                {tGroup(currentGroup)}
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
                            // 核心修改 2：文件的右键事件必须阻止冒泡
                            onContextMenu={(e) => {
                                e.preventDefault();
                                e.stopPropagation(); // 阻止事件冒泡到外层容器，避免同时触发空白菜单
                                if (onContextMenu) {
                                    onContextMenu(e, file);
                                }
                            }}
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
                                {isImg && !hasImageFailed ? (
                                ) : null}

                                {/* 逻辑修复：当不是图片，或者图片加载失败时，渲染对应的标准矢量图标 */}
                                {(!isImg || hasImageFailed) && (
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
        case 'png': case 'jpg': case 'jpeg': case 'gif': case 'webp': case 'svg': case 'bmp': return 'image';
        case 'mp3': case 'wav': case 'flac': case 'ogg': return 'audio_file';
        case 'mp4': case 'mkv': case 'avi': case 'mov': return 'movie';
        case 'pdf': return 'picture_as_pdf';
        case 'txt': case 'md': case 'json': case 'conf': return 'article';
        case 'zip': case 'tar': case 'gz': case '7z': case 'rar': return 'folder_zip';
        default: return 'insert_drive_file';
    }                                    <img
                                        src={`media://${file.path}`}
                                        alt={file.name}
                                        className="file-thumbnail"
                                        // 性能优化：为大量图片添加懒加载和异步解码
                                        loading="lazy"
                                        decoding="async"
                                        onError={() => {
                                            // 图片加载失败时，将其加入失败队列，触发重绘降级为标准图标
                                            setFailedImages(prev => {
                                                const next = new Set(prev);
                                                next.add(file.path);
                                                return next;
                                            });
                                        }}
                                        style={{
                                            width: viewMode === 'list' ? `${iconSize}px` : '100%',
                                            height: viewMode === 'list' ? `${iconSize}px` : '100%',
                                            objectFit: 'cover' // 保持缩略图比例
                                        }}
                                    />

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
