import React, { useEffect, useRef } from 'react';
import './ContextMenu.css';
import { Icon } from './Icon';

export interface ContextMenuItem {
    label: string;
    icon?: string;
    action: () => void;
    shortcut?: string;
    divider?: boolean;
}

interface ContextMenuProps {
    x: number;
    y: number;
    items: ContextMenuItem[];
    onClose: () => void;
}

export const ContextMenu: React.FC<ContextMenuProps> = ({ x, y, items, onClose }) => {
    const menuRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const handleClickOutside = (event: MouseEvent) => {
            if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
                onClose();
            }
        };

        // Slight delay to avoid immediate close from the trigger click
        setTimeout(() => {
            document.addEventListener('click', handleClickOutside);
        }, 0);

        return () => {
            document.removeEventListener('click', handleClickOutside);
        };
    }, [onClose]);

    useEffect(() => {
        // Adjust position if out of bounds
        if (menuRef.current) {
            const { innerWidth, innerHeight } = window;
            const rect = menuRef.current.getBoundingClientRect();

            let newX = x;
            let newY = y;

            if (x + rect.width > innerWidth) {
                newX = x - rect.width;
            }
            if (y + rect.height > innerHeight) {
                newY = y - rect.height;
            }

            menuRef.current.style.left = `${newX}px`;
            menuRef.current.style.top = `${newY}px`;
        }
    }, [x, y]);

    return (
        <div
            ref={menuRef}
            className="context-menu"
            style={{ left: x, top: y }}
        >
            {items.map((item, index) => (
                item.divider ? (
                    <div key={index} className="context-menu-divider" />
                ) : (
                    <button key={index} className="context-menu-item" onClick={() => {
                        item.action();
                        onClose();
                    }}>
                        {item.icon && <Icon name={item.icon} className="context-menu-icon" />}
                        <span className="context-menu-label">{item.label}</span>
                        {item.shortcut && <span className="context-menu-shortcut">{item.shortcut}</span>}
                    </button>
                )
            ))}
        </div>
    );
};
