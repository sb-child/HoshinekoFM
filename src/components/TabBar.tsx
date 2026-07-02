import React from 'react';
import { Icon } from './Icon';
import { t } from '../i18n';
import './TabBar.css';

interface Tab {
    id: string;
    title: string;
}

interface TabBarProps {
    tabs: Tab[];
    activeTabId: string;
    onTabClick: (id: string) => void;
    onTabClose: (id: string) => void;
    onNewTab: () => void;
}

const getTabTitle = (title: string): string => {
  const normalizeTitle = title.toLowerCase();

  switch (normalizeTitle) {
  case 'dashboard':
  case 'app://dashboard':
    return t('tab.dashboard');
  case 'home':
    return t('tab.home');
  case 'downloads':
    return t('tab.downloads');
  case 'documents':
    return t('tab.documents');
  case 'music':
    return t('tab.music');
  case 'pictures':
    return t('tab.pictures');
  case 'videos':
    return t('tab.videos');
  default:
    return title;
  }
};

export const TabBar: React.FC<TabBarProps> = ({ tabs, activeTabId, onTabClick, onTabClose, onNewTab }) => {
  return (
    <div className="tab-bar">
      {tabs.map(tab => (
        <div
          key={tab.id}
          className={`tab-item ${tab.id === activeTabId ? 'active' : ''}`}
          onClick={() => onTabClick(tab.id)}
        >
          <span className="tab-title">{getTabTitle(tab.title)}</span>
          <button
            className="tab-close-btn"
            onClick={(e) => {
              e.stopPropagation();
              onTabClose(tab.id);
            }}
          >
            <Icon name="close" style={{ fontSize: '16px' }} />
          </button>
        </div>
      ))}
      <button className="new-tab-btn" onClick={onNewTab}>
        <Icon name="add" />
      </button>
    </div>
  );
};
