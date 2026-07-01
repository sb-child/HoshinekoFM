# AGENTS.md — Material 3 File Explorer

## Commands

| Command | What it does |
|---|---|
| `npm run dev` | Start Vite dev server (port 5173) |
| `npm run electron:dev` | Start Vite + wait-on port 5173 + launch Electron with devtools |
| `npm run build` | `tsc -b && vite build && tsc -p electron/tsconfig.json` |
| `npm run electron:build` | `npm run build` → `set-cjs.cjs` → `electron-builder` (outputs AppImage) |
| `npm run lint` | `eslint .` (flat config, ESLint v9) |

No `typecheck` script — run `npx tsc -b` or `npx tsc --noEmit` for type-checking.

No test files or test scripts exist in the repository.

## Architecture

- **Frontend**: `src/main.tsx` → `App.tsx`, React + Vite, ESM
- **Backend**: `electron/main.ts`, `electron/preload.ts`, `electron/pty.ts` — compiled to `dist-electron/` as **CommonJS** (`electron/tsconfig.json` sets `"module": "commonjs"`)
- **IPC bridge**: `preload.ts` exposes `window.electron` via `contextBridge`; types in `src/types/electron.d.ts`
- **Services**: `FileSystemService`, `ThemeService`, `TerminalService` live in `src/services/`
- **Terminal**: `node-pty` spawned via `electron/pty.ts`; frontend communicates via `window.electron.ptySpawn/ptyWrite/ptyOnData`
- **No routing**: Tab/explorer state managed in `App.tsx` via `useState`; no React Router

## Commands order for production build

`npm run build` runs: `tsc -b` (frontend) → `vite build` → `tsc -p electron/tsconfig.json` (electron). The `electron:build` command additionally runs `scripts/set-cjs.cjs` (writes `dist-electron/package.json` with `{"type":"commonjs"}`) and then `electron-builder`.

## Key quirks

- **Electron tsconfig is separate**: `electron/tsconfig.json` uses CommonJS and outputs to `dist-electron/`. Do not use `verbatimModuleSyntax` there.
- **`set-cjs.cjs` is required**: Without it, Electron errors on `import` statements because the compiled electron JS is ESModule-like but Electron's main process needs CJS.
- **`vite.config.ts` excludes `react-window` and `react-virtualized-auto-sizer`** from dependency optimization.
- **File operations use Linux system commands**: `du -sb`, `find`, `unzip`, `tar`, `lsblk`, `df`, `xdg-mime`, `grep`. Not portable to macOS/Windows.
- **No test framework** — none installed, no test files.
- **Dynamic theming**: reads `~/.config/matugen/theme.css` at startup.
- **Product name**: "Materials" (not the npm package name `material-3-file-manager`).
- **CSS only**: no CSS-in-JS or CSS modules — plain `.css` files imported directly into components.
- **Monorepo workspace**: `pnpm-workspace.yaml` exists but is only used for `allowBuilds` lists (no actual packages). Both `package-lock.json` and `pnpm-lock.yaml` are checked in.

## Style conventions

- **Imports**: React imports use `import { ... } from 'react'` pattern.
- **TypeScript**: `verbatimModuleSyntax: true` in frontend tsconfig — use `import type` for type-only imports.
- **`noUnusedLocals` / `noUnusedParameters`**: both enabled in frontend tsconfig — unused vars/params cause errors.
- **CSS**: flat `.css` files in same directory as component, imported in component file.
