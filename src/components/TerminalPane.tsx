import React, { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';

interface TerminalPaneProps {
    cwd?: string;
    onClose?: () => void;
}

export const TerminalPane: React.FC<TerminalPaneProps> = ({ cwd }) => {
    const containerRef = useRef<HTMLDivElement>(null);
    const terminalRef = useRef<Terminal | null>(null);
    const fitAddonRef = useRef<FitAddon | null>(null);
    const pidRef = useRef<number | null>(null);

    useEffect(() => {
        if (!containerRef.current) return;

        // 保持你原本完全正确的同步初始化逻辑
        const term = new Terminal({
            cursorBlink: true,
            fontFamily: 'Consolas, monospace',
            fontSize: 14,
            theme: {
                background: '#1e1e1e',
                foreground: '#ffffff'
            }
        });

        const fitAddon = new FitAddon();
        term.loadAddon(fitAddon);
        term.open(containerRef.current);
        fitAddon.fit();

        terminalRef.current = term;
        fitAddonRef.current = fitAddon;

        // 保持你原本完全打通提示符的 Promise 底层链式结构不变
        window.electron.ptySpawn(cwd || '').then((pid) => {
            pidRef.current = pid;

            // 进程就绪后，立刻聚焦终端
            term.focus();

            // Handle incoming data
            const cleanup = window.electron.ptyOnData(pid, (data: string) => {
                term.write(data);
            });

            // Handle exit
            window.electron.ptyOnExit(pid, () => {
                term.write('\r\nProcess exited.\r\n');
                cleanup();
                pidRef.current = null;
            });

            return cleanup;
        }).then((cleanup) => {
            // Send input to PTY
            term.onData((data: string) => {
                if (pidRef.current) {
                    window.electron.ptyWrite(pidRef.current, data);
                }
            });

            // Handle resize
            const handleResize = () => {
                fitAddon.fit();
                if (pidRef.current) {
                    window.electron.ptyResize(pidRef.current, term.cols, term.rows);
                }
            };

            window.addEventListener('resize', handleResize);
            term.onResize((size: { cols: number; rows: number }) => {
                if (pidRef.current) {
                    window.electron.ptyResize(pidRef.current, size.cols, size.rows);
                }
            });

            // Initial resize sync
            if (pidRef.current) {
                window.electron.ptyResize(pidRef.current, term.cols, term.rows);
            }

            return () => {
                window.removeEventListener('resize', handleResize);
                cleanup && cleanup();
                term.dispose();
                if (pidRef.current) window.electron.ptyKill(pidRef.current);
            };
        });

        return () => {};
    }, []); // Only mount once

    useEffect(() => {
        if (cwd && pidRef.current) {
            const safePath = cwd.replace(/'/g, "'\\''");
            const cmd = `cd '${safePath}'\r`; 
            window.electron.ptyWrite(pidRef.current, cmd);
        }
    }, [cwd]);

    return (
        <div 
            style={{ 
                width: '100%', 
                height: '100%', 
                background: '#1e1e1e', 
                overflow: 'hidden',
                position: 'relative',
                zIndex: 10 // 提高层级，防止点击事件穿透到下方的文件列表背景上
            }} 
            ref={containerRef} 
            // 在鼠标按下阶段截击，阻止事件向上传播给文件浏览器背景，从而保住焦点
            onMouseDown={(e) => {
                e.stopPropagation();
            }}
            // 点击黑框的任意地方时，强制让 xterm.js 内部的隐藏输入域重新获取焦点
            onClick={(e) => {
                e.stopPropagation();
                if (terminalRef.current) {
                    terminalRef.current.focus();
                }
            }}
            // 阻止键盘输入事件向外泄露，防止触发文件浏览器的全局快捷键
            onKeyDown={(e) => {
                e.stopPropagation();
            }}
        />
    );
};
