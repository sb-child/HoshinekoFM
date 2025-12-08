import React from 'react';
import { IconButton } from './IconButton';
import './NavigationRail.css';

interface NavigationItem {
    icon: React.ReactNode;
    activeIcon?: React.ReactNode;
    label?: string;
    onClick?: () => void;
    active?: boolean;
}

interface NavigationRailProps {
    items: NavigationItem[];
    fab?: React.ReactNode;
}

export const NavigationRail: React.FC<NavigationRailProps> = ({ items, fab }) => {
    return (
        <nav className="m3-navigation-rail">
            {fab && <div className="m3-navigation-rail__fab">{fab}</div>}
            <div className="m3-navigation-rail__menu">
                {items.map((item, index) => (
                    <div key={index} className="m3-navigation-rail__item">
                        <IconButton
                            variant={item.active ? 'filled' : 'standard'}
                            selected={item.active}
                            onClick={item.onClick}
                            aria-label={item.label}
                        >
                            {item.active && item.activeIcon ? item.activeIcon : item.icon}
                        </IconButton>
                        {item.label && <span className="m3-navigation-rail__label">{item.label}</span>}
                    </div>
                ))}
            </div>
        </nav>
    );
};
