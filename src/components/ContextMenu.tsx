import React, { useState, useEffect, useRef } from 'react';
import './ContextMenu.css';
import { Icon } from './Icon';
import { t } from '../i18n';
import type zhCN from '../i18n/zh-CN';

type I18nKey = keyof typeof zhCN;

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

const MENU_PADDING = 8;

function clampPosition(x: number, y: number, width: number, height: number) {
  const { innerWidth, innerHeight } = window;
  let newX = x;
  let newY = y;

  if (x + width > innerWidth) newX = x - width;
  if (y + height > innerHeight) newY = y - height;
  if (newX < MENU_PADDING) newX = MENU_PADDING;
  if (newY < MENU_PADDING) newY = MENU_PADDING;

  return { left: newX, top: newY };
}

const labelToKey: Record<string, string> = {
  'Open': 'context_menu.open',
  'Open With...': 'context_menu.open_with',
  'Open in Terminal': 'context_menu.open_terminal',
  'Copy': 'context_menu.copy',
  'Cut': 'context_menu.cut',
  'Paste': 'context_menu.paste',
  'Rename': 'context_menu.rename',
  'Delete': 'context_menu.delete',
  'Properties': 'context_menu.properties',
  'New Folder': 'context_menu.new_folder',
  'New File': 'context_menu.new_file',
  'Refresh': 'context_menu.refresh',
  'Select All': 'context_menu.select_all',
  'Pin to Dashboard': 'context_menu.pin',
  'Unpin from Dashboard': 'context_menu.unpin',
  'Extract Here': 'context_menu.extract_here',
};

export const ContextMenu: React.FC<ContextMenuProps> = ({ x, y, items, onClose }) => {
  const menuRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number }>({ left: x, top: y });

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        onClose();
      }
    };
    setTimeout(() => document.addEventListener('click', handleClickOutside), 0);
    return () => document.removeEventListener('click', handleClickOutside);
  }, [onClose]);

  useEffect(() => {
    if (!menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    setPos(clampPosition(x, y, rect.width, rect.height));

    const handleResize = () => {
      if (!menuRef.current) return;
      const r = menuRef.current.getBoundingClientRect();
      setPos(clampPosition(x, y, r.width, r.height));
    };
    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, [x, y]);

  const processedItems = items.map(item => {
    if (item.divider) return item;

    if (item.label === 'Rename') {
      const originalAction = item.action;
      return {
        ...item,
        action: () => {
          originalAction();
          setTimeout(() => {
            const dialogTitles = document.querySelectorAll('.md3-dialog-title, .dialog-title, h2');
            dialogTitles.forEach(el => {
              if (el.textContent === 'Rename') el.textContent = t('dialog.rename.title');
            });
            const buttons = document.querySelectorAll('button');
            buttons.forEach(btn => {
              if (btn.textContent === 'Cancel') btn.textContent = t('dialog.rename.cancel');
              if (btn.textContent === 'Rename') btn.textContent = t('dialog.rename.confirm');
            });
            const inputs = document.querySelectorAll('input');
            inputs.forEach(input => {
              if (input.placeholder === 'New name') input.placeholder = t('dialog.rename.placeholder');
            });
          }, 50);
        }
      };
    }

    return item;
  });

  return (
    <div
      ref={menuRef}
      className="context-menu"
      style={{ left: pos.left, top: pos.top }}
    >
      {processedItems.map((item, index) => (
        item.divider ? (
          <div key={index} className="context-menu-divider" />
        ) : (
          <button key={index} className="context-menu-item" onClick={() => {
            item.action();
            onClose();
          }}>
            {item.icon && <Icon name={item.icon} className="context-menu-icon" />}
            <span className="context-menu-label">
              {labelToKey[item.label] ? t(labelToKey[item.label] as I18nKey) : item.label}
            </span>
            {item.shortcut && <span className="context-menu-shortcut">{item.shortcut}</span>}
          </button>
        )
      ))}
    </div>
  );
};
