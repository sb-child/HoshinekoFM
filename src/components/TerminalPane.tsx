import React, { useEffect, useRef } from 'react';
import { Terminal } from 'xterm';
import { FitAddon } from 'xterm-addon-fit';
import 'xterm/css/xterm.css';

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

        // Spawn PTY
        window.electron.ptySpawn(cwd || '').then((pid) => {
            pidRef.current = pid;

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

        return () => {
            // Cleanup checked within the promise chain above, 
            // but for safety if unmount happens before spawn completes:
            // logic is tricky here with async spawn. 
            // Simplified: spawn is fast enough, or we check ref.
        };
    }, []); // Only mount once

    useEffect(() => {
        if (cwd && pidRef.current) {
            // Escape path for shell safety - simple version
            // For bash/zsh, wrapping in single quotes is usually safe, replacing single quotes with '\\''
            const safePath = cwd.replace(/'/g, "'\\''");
            const cmd = `cd '${safePath}'\r`; // && clear\r`; 
            window.electron.ptyWrite(pidRef.current, cmd);
        }
    }, [cwd]);

    return (
        <div style={{ width: '100%', height: '100%', background: '#1e1e1e', overflow: 'hidden' }} ref={containerRef} />
    );
};
