# AGENTS.md — Hoshineko File Manager

## Commands

| Command                  | What it does                                                                                                                                             |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run dev`            | Start Vite dev server (port 5173)                                                                                                                        |
| `npm run electron:dev`   | Start Vite + `build-native.cjs` + build electron CJS + `set-cjs.cjs` + `copy-native.cjs` + launch Electron with devtools                                |
| `npm run build`          | `tsc -b` → `vite build` → `build-native.cjs` → `tsc -p electron/tsconfig.json` → `copy-native.cjs`                                                      |
| `npm run electron:build` | `npm run build` → `scripts/set-cjs.cjs` → `electron-builder` (outputs AppImage to `release/`)                                                            |
| `npm run lint`           | `eslint .` (flat config, ESLint v9)                                                                                                                      |

No `typecheck` script — run `npx tsc -b` or `npx tsc --noEmit` for type-checking.
No test framework, no test files.

### Native addon build scripts

| Script                       | What it does                                                                                   |
| ---------------------------- | ---------------------------------------------------------------------------------------------- |
| `node scripts/build-native.cjs` | Compile `native/` C++ addon against Electron's Node.js ABI (Linux only, uses `node-gyp`)   |
| `node scripts/copy-native.cjs`  | Copy `linux_dnd.node` from `native/build/Release/` into `dist-electron/` (Linux only)      |

Both scripts exit silently on non-Linux or when GTK4 is missing — no error, no build failure.

## Architecture

- **Frontend**: `src/main.tsx` → `App.tsx`, React 19 + Vite, ESM
- **UI framework**: `@material/web` (Lit-based Material 3 web components) — all web components registered in `src/material-web.ts` (imported before App)
- **Backend**: `electron/main.ts`, `electron/preload.ts`, `electron/pty.ts`, `electron/handlers/{fs,system,window}.ts` — compiled to `dist-electron/` as **CommonJS** (`electron/tsconfig.json` sets `"module": "commonjs"`)
- **Native addon**: `native/src/linux_dnd.cc` — GDK4-based drag-and-drop addon for Linux (replaces broken Electron `startDrag()` on Wayland). Compiled via `node-gyp`, loaded in `electron/handlers/window.ts` at runtime with graceful fallback.
- **IPC bridge**: `preload.ts` exposes `window.electron` via `contextBridge`; types in `src/types/electron.d.ts`
- **Services**: `FileSystemService`, `ThemeService` live in `src/services/`
- **Terminal**: `node-pty` spawned via `electron/pty.ts`; frontend communicates via `window.electron.ptySpawn/ptyWrite/ptyOnData/ptyOnExit`
- **No routing**: Tab/explorer state managed in `App.tsx` via `useState`; no React Router

## Commands order for production build

`npm run build` runs: `tsc -b` (frontend, project references) → `vite build` → `scripts/build-native.cjs` (compile native addon for Electron ABI) → `tsc -p electron/tsconfig.json` (electron CJS) → `scripts/copy-native.cjs` (copy .node to dist-electron/).
`electron:build` additionally runs `scripts/set-cjs.cjs` (writes `dist-electron/package.json` with `{"type":"commonjs"}`) then `electron-builder`.

## Key quirks

- **Electron tsconfig is separate**: `electron/tsconfig.json` uses CommonJS, outputs to `dist-electron/`. Do not use `verbatimModuleSyntax` there.
- **`scripts/set-cjs.cjs` is required**: Without it, Electron errors on `import` statements because the compiled electron JS uses `import` syntax but Electron's main process needs CJS.
- **Custom `media://` protocol**: Registered in `electron/main.ts` for serving file thumbnails (including generated thumbnails) via `net.fetch`. Paths are served as `media://<absolute-path>`.
- **`vite.config.ts` uses `base: './'`**: Required for Electron `file://` loading of production builds (otherwise asset paths break).
- **`vite.config.ts` excludes `react-window` and `react-virtualized-auto-sizer`** from dependency optimization.
- **File operations use Linux system commands**: `du -sb`, `find`, `unzip`, `tar`, `lsblk`, `df`, `xdg-mime`, `grep`. Not portable to macOS/Windows.
- **UDISKS2 device monitoring**: `setupUdisks2Monitor` in `electron/handlers/system.ts` listens for device add/remove via D-Bus; only works on Linux.
- **Dynamic theming**: reads `~/.config/matugen/theme.css` at startup via `theme:get-css` IPC.
- **Product name**: "Materials" (`productName` in `package.json`), not the npm package name `material-3-file-manager`.
- **CSS only**: no CSS-in-JS or CSS modules — plain `.css` files in same directory as component, imported in component file.
- **Monorepo workspace**: `pnpm-workspace.yaml` exists but only for `allowBuilds` hints (no actual packages). Both `package-lock.json` and `pnpm-lock.yaml` are checked in.
- **Native DnD addon (`native/`)**: GDK4-based C++ addon that replaces Electron's broken `startDrag()` on Linux Wayland. Compiled via `node-gyp` against Electron's Node.js headers (must match Electron version). Falls back to Electron's built-in `startDrag()` on non-Linux or if compilation fails. GTK4 headers (`libgtk-4-dev` / `gtk4-devel`) required at build time, not runtime.

## Style conventions

- **Indentation**: ESLint enforces 2-space indent (`indent: ['error', 2]`)
- **Imports**: React imports use `import { ... } from 'react'` pattern.
- **TypeScript frontend**: `verbatimModuleSyntax: true` in `tsconfig.app.json` — use `import type` for type-only imports.
- **`noUnusedLocals` / `noUnusedParameters`**: both enabled in frontend tsconfig — unused vars/params cause compile errors.
- **CSS**: flat `.css` files in same directory as component, imported in component file.
- **Web Components**: Material 3 UI uses `@material/web` custom elements (e.g., `<md-filled-button>`, `<md-dialog>`). Import any new component in `src/material-web.ts`.

## 注意事项

一定要写jsdoc！写字段说明！！！写i18n！！

先检查会不会破坏别的代码逻辑和功能。会的话跟我说！！
