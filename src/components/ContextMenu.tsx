import React, { useState, useEffect, useRef } from 'react';
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

// 统一汉化词典（包含全量右键菜单、解压、打开方式等）
const contextLocaleMap: Record<string, string> = {
  'Open': '打开',
  'Open in Terminal': '在内置终端打开',
  'Copy': '复制',
  'Cut': '剪切',
  'Paste': '粘贴',
  'Rename': '重命名',
  'Delete': '删除',
  'Properties': '属性',
  'New Folder': '新建文件夹',
  'New File': '新建文件',
  'Refresh': '刷新',
  'Select All': '全选',
  'Pin to Dashboard': '固定到仪表盘',
  'Unpin from Dashboard': '从仪表盘取消固定',
  'Extract Here': '解压到当前文件夹',
  'Open With...': '打开方式...'
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
              if (el.textContent === 'Rename') el.textContent = '重命名';
            });
            const buttons = document.querySelectorAll('button');
            buttons.forEach(btn => {
              if (btn.textContent === 'Cancel') btn.textContent = '取消';
              if (btn.textContent === 'Rename') btn.textContent = '重命名';
            });
            const inputs = document.querySelectorAll('input');
            inputs.forEach(input => {
              if (input.placeholder === 'New name') input.placeholder = '新名称';
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
              {contextLocaleMap[item.label] || item.label}
            </span>
            {item.shortcut && <span className="context-menu-shortcut">{item.shortcut}</span>}
          </button>
        )
      ))}
    </div>
  );
};
