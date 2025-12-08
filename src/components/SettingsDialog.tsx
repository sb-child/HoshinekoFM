import React from 'react';
import { Dialog } from './Dialog';
import { Button } from './Button';
import { Icon } from './Icon';

interface SettingsDialogProps {
    open: boolean;
    onClose: () => void;
    showHiddenFiles: boolean;
    onToggleHiddenFiles: () => void;
    iconSize: number;
    onIconSizeChange: (size: number) => void;
    viewMode: 'grid' | 'list';
    onViewModeChange: (mode: 'grid' | 'list') => void;
    filledIcons: boolean;
    onToggleFilledIcons: () => void;
    onImportCss: () => void;
    customCssPath?: string;
}

export const SettingsDialog: React.FC<SettingsDialogProps> = ({
    open, onClose,
    showHiddenFiles, onToggleHiddenFiles,
    iconSize, onIconSizeChange,
    viewMode, onViewModeChange,
    filledIcons, onToggleFilledIcons,
    onImportCss, customCssPath
}) => {
    return (
        <Dialog
            title="Settings"
            open={open}
            onClose={onClose}
            actions={
                <Button onClick={onClose} variant="filled">Done</Button>
            }
        >
            <div style={{ padding: '0 8px', minWidth: '300px' }}>
                <div
                    onClick={onToggleHiddenFiles}
                    style={{
                        display: 'flex', alignItems: 'center', justifyContent: 'space-between',
                        padding: '12px 0', cursor: 'pointer', userSelect: 'none'
                    }}
                >
                    <div style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
                        <Icon name={showHiddenFiles ? 'visibility' : 'visibility_off'} />
                        <div style={{ fontSize: '16px' }}>Show Hidden Files</div>
                    </div>
                    {/* Material 3 Switch */}
                    <div style={{
                        width: '52px', height: '32px',
                        background: showHiddenFiles ? 'var(--md-sys-color-primary)' : 'var(--md-sys-color-surface-container-highest)',
                        borderRadius: '16px', position: 'relative', transition: 'background 0.2s',
                        border: `2px solid ${showHiddenFiles ? 'var(--md-sys-color-primary)' : 'var(--md-sys-color-outline)'}`
                    }}>
                        <div style={{
                            width: '16px', height: '16px', borderRadius: '50%',
                            background: showHiddenFiles ? 'var(--md-sys-color-on-primary)' : 'var(--md-sys-color-outline)',
                            position: 'absolute', top: '50%', transform: `translate(${showHiddenFiles ? '28px' : '6px'}, -50%)`,
                            transition: 'transform 0.2s, background 0.2s'
                        }} />
                    </div>
                </div>

                <div style={{ padding: '12px 0', borderTop: '1px solid var(--md-sys-color-outline-variant)' }}>
                    <div style={{ fontSize: '14px', color: 'var(--md-sys-color-primary)', fontWeight: 500, marginBottom: '8px' }}>Appearance</div>

                    {/* View Mode */}
                    <div style={{ marginBottom: '16px' }}>
                        <div style={{ marginBottom: '8px' }}>View Mode</div>
                        <div style={{ display: 'flex', gap: '8px' }}>
                            <Button
                                variant={viewMode === 'grid' ? 'filled' : 'outlined'}
                                onClick={() => onViewModeChange('grid')}
                            >
                                <Icon name="grid_view" /> Grid
                            </Button>
                            <Button
                                variant={viewMode === 'list' ? 'filled' : 'outlined'}
                                onClick={() => onViewModeChange('list')}
                            >
                                <Icon name="view_list" /> List
                            </Button>
                        </div>
                    </div>

                    {/* Icon Size */}
                    <div style={{ marginBottom: '16px' }}>
                        <div style={{ marginBottom: '8px', display: 'flex', justifyContent: 'space-between' }}>
                            <span>Icon Size</span>
                            <span>{iconSize}px</span>
                        </div>
                        <input
                            type="range"
                            min="32"
                            max="128"
                            step="8"
                            value={iconSize}
                            onChange={(e) => onIconSizeChange(Number(e.target.value))}
                            style={{ width: '100%', accentColor: 'var(--md-sys-color-primary)' }}
                        />
                    </div>

                    {/* Filled Icons */}
                    <div
                        onClick={onToggleFilledIcons}
                        style={{
                            display: 'flex', alignItems: 'center', justifyContent: 'space-between',
                            cursor: 'pointer', userSelect: 'none'
                        }}
                    >
                        <div style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
                            <Icon name="favorite" filled={filledIcons} />
                            <div style={{ fontSize: '16px' }}>Filled Icons</div>
                        </div>
                        <div style={{
                            width: '52px', height: '32px',
                            background: filledIcons ? 'var(--md-sys-color-primary)' : 'var(--md-sys-color-surface-container-highest)',
                            borderRadius: '16px', position: 'relative', transition: 'background 0.2s',
                            border: `2px solid ${filledIcons ? 'var(--md-sys-color-primary)' : 'var(--md-sys-color-outline)'}`
                        }}>
                            <div style={{
                                width: '16px', height: '16px', borderRadius: '50%',
                                background: filledIcons ? 'var(--md-sys-color-on-primary)' : 'var(--md-sys-color-outline)',
                                position: 'absolute', top: '50%', transform: `translate(${filledIcons ? '28px' : '6px'}, -50%)`,
                                transition: 'transform 0.2s, background 0.2s'
                            }} />
                        </div>
                    </div>
                </div>

                <div style={{ padding: '12px 0', borderTop: '1px solid var(--md-sys-color-outline-variant)' }}>
                    <div style={{ fontSize: '14px', color: 'var(--md-sys-color-primary)', fontWeight: 500, marginBottom: '8px' }}>Customization</div>
                    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                        <div>
                            <div style={{ fontSize: '16px' }}>Custom CSS</div>
                            {customCssPath && <div style={{ fontSize: '12px', color: 'var(--md-sys-color-on-surface-variant)', maxWidth: '200px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{customCssPath}</div>}
                        </div>
                        <Button
                            variant="outlined"
                            onClick={onImportCss}
                        >
                            Import CSS
                        </Button>
                    </div>
                </div>
            </div>
        </Dialog>
    );
};

