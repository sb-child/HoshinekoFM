import React, { useEffect, useRef, useState, useCallback } from 'react';
import './Toast.css';

export type ToastType = 'success' | 'error' | 'info';

interface ToastProps {
    message: string;
    type: ToastType;
    onClose: () => void;
    duration?: number;
}

export const Toast: React.FC<ToastProps> = ({ message, type, onClose, duration = 3000 }) => {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startTimeRef = useRef<number>(Date.now());
  const pausedRef = useRef(false);
  const [hovered, setHovered] = useState(false);

  const scheduleClose = useCallback((delay: number) => {
    if (timerRef.current) clearTimeout(timerRef.current);
    startTimeRef.current = Date.now();
    timerRef.current = setTimeout(onClose, delay);
  }, [onClose]);

  useEffect(() => {
    startTimeRef.current = Date.now();
    timerRef.current = setTimeout(onClose, duration);
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [duration, onClose]);

  const handleMouseEnter = () => {
    setHovered(true);
    pausedRef.current = true;
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  };

  const handleMouseLeave = () => {
    setHovered(false);
    pausedRef.current = false;
    const elapsed = Date.now() - startTimeRef.current;
    const remaining = Math.max(duration - elapsed, 0);
    scheduleClose(remaining);
  };

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(message);
    } catch {
      // ignore clipboard errors
    }
  };

  return (
    <div
      className={`toast toast-${type}${hovered ? ' toast-hovered' : ''}`}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      <span className="toast-message">{message}</span>
      <button className="toast-copy-btn" onClick={handleCopy} title="复制">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
        </svg>
      </button>
    </div>
  );
};
