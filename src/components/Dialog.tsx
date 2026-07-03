import React, { useRef, useCallback } from 'react';
import { Dialog as MdDialog } from './md';

const SCROLLBAR_STYLE_ID = 'md-dialog-scrollbar-style';

const SCROLLBAR_CSS = `
.scroller::-webkit-scrollbar {
  width: 8px;
  height: 8px;
}
.scroller::-webkit-scrollbar-track {
  background: transparent;
}
.scroller::-webkit-scrollbar-thumb {
  background: var(--md-sys-color-outline-variant);
  border-radius: 4px;
}
.scroller::-webkit-scrollbar-thumb:hover {
  background: var(--md-sys-color-outline);
}
`;

function injectScrollbarStyle(root: ShadowRoot) {
  if (root.getElementById(SCROLLBAR_STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = SCROLLBAR_STYLE_ID;
  style.textContent = SCROLLBAR_CSS;
  root.appendChild(style);
}

interface DialogProps {
  title: string;
  open: boolean;
  onClose: () => void;
  children: React.ReactNode;
  actions?: React.ReactNode;
}

export const Dialog: React.FC<DialogProps> = ({ title, open, onClose, children, actions }) => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const dialogRef = useRef<any>(null);

  const handleOpened = useCallback(() => {
    const el = dialogRef.current;
    if (!el) return;
    const root = el.shadowRoot;
    if (root) injectScrollbarStyle(root);
  }, []);

  return (
    <MdDialog
      ref={dialogRef}
      open={open}
      onCancel={onClose}
      onClose={onClose}
      onOpened={handleOpened}
    >
      <span slot="headline">{title}</span>
      <div slot="content">{children}</div>
      {actions && <div slot="actions">{actions}</div>}
    </MdDialog>
  );
};
