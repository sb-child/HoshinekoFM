import React from 'react';
import {
  IconButton as MdIconButton,
  FilledIconButton,
  TonalIconButton,
  OutlinedIconButton,
} from './md';

interface IconButtonProps {
  variant?: 'filled' | 'tonal' | 'outlined' | 'standard';
  toggle?: boolean;
  selected?: boolean;
  disabled?: boolean;
  type?: 'button' | 'submit' | 'reset';
  className?: string;
  style?: React.CSSProperties;
  title?: string;
  onClick?: React.MouseEventHandler<HTMLElement>;
  onDragOver?: React.DragEventHandler<HTMLElement>;
  onDragEnter?: React.DragEventHandler<HTMLElement>;
  onDragLeave?: React.DragEventHandler<HTMLElement>;
  onDrop?: React.DragEventHandler<HTMLElement>;
  onContextMenu?: React.MouseEventHandler<HTMLElement>;
  children?: React.ReactNode;
  tabIndex?: number;
  id?: string;
}

const variantMap = {
  standard: MdIconButton,
  filled: FilledIconButton,
  tonal: TonalIconButton,
  outlined: OutlinedIconButton,
} as const;

export const IconButton: React.FC<IconButtonProps> = ({
  variant = 'standard',
  selected,
  toggle,
  children,
  className = '',
  disabled,
  type,
  style,
  title,
  onClick,
  onDragOver,
  onDragEnter,
  onDragLeave,
  onDrop,
  onContextMenu,
  tabIndex,
  id,
}) => {
  const Component = variantMap[variant];
  return (
    <Component
      className={className || undefined}
      disabled={disabled}
      toggle={toggle}
      selected={selected}
      type={type ?? 'button'}
      style={style}
      title={title}
      onClick={onClick}
      onDragOver={onDragOver}
      onDragEnter={onDragEnter}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
      onContextMenu={onContextMenu}
      tabIndex={tabIndex}
      id={id}
    >
      {children}
    </Component>
  );
};
