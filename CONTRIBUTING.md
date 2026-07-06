# Contributing to Hoshineko File Manager

## Prerequisites

| Dependency | Fedora | Ubuntu/Debian | Arch |
|-----------|--------|---------------|------|
| Node.js 22+ | `nodejs` | `nodejs` | `nodejs` |
| npm | `npm` | `npm` | `npm` |
| GTK4 headers | `gtk4-devel` | `libgtk-4-dev` | `gtk4` |

```bash
# Fedora
sudo dnf install nodejs npm gtk4-devel

# Ubuntu/Debian
sudo apt install nodejs npm libgtk-4-dev

# Arch
sudo pacman -S nodejs npm gtk4
```

GTK4 headers are only needed at build time for the native drag-and-drop addon. The app runs fine without them (DnD falls back to Electron built-in).

## Development

```bash
# 1. Clone and install
git clone <repo-url>
cd HoshinekoFM
npm install
cd native && npm install && cd ..

# 2. Start dev mode (Vite + Electron with devtools)
npm run electron:dev
```

### What `npm run electron:dev` does

1. Starts Vite dev server on port 5173
2. Waits for Vite to be ready
3. Builds the native GDK4 addon for Electron's Node.js ABI (`scripts/build-native.cjs`)
4. Compiles Electron TypeScript (`tsc -p electron/tsconfig.json`)
5. Writes `dist-electron/package.json` with `"type": "commonjs"` (`scripts/set-cjs.cjs`)
6. Copies the native addon to `dist-electron/` (`scripts/copy-native.cjs`)
7. Launches Electron with devtools open

Changes to frontend code are hot-reloaded by Vite. Changes to Electron code require restarting `electron:dev`.

## Project structure

```
HoshinekoFM/
├── src/                     # React frontend (ESM)
│   ├── components/          # UI components
│   ├── contexts/            # React contexts (DragContext, etc.)
│   ├── services/            # FileSystemService, ThemeService
│   ├── types/               # TypeScript type definitions
│   └── utils/               # Utilities (dragIconRenderer, etc.)
├── electron/                # Electron main process (compiled to CJS)
│   ├── handlers/            # IPC handlers (fs, system, window)
│   ├── main.ts              # App entry point
│   ├── preload.ts           # contextBridge API
│   └── pty.ts               # Terminal (node-pty) management
├── native/                  # Native C++ addon (Linux DnD)
│   ├── binding.gyp          # node-gyp build config
│   └── src/linux_dnd.cc     # GDK4 drag-and-drop implementation
├── scripts/                 # Build scripts
│   ├── build-native.cjs     # Compile native addon
│   ├── copy-native.cjs      # Copy .node to dist-electron/
│   └── set-cjs.cjs          # Write dist-electron/package.json
├── assets/                  # App icons
└── dist-electron/           # Compiled Electron output (generated)
```

## Building a release

```bash
npm run electron:build
```

Output: `release/Materials-<version>.AppImage`

### Build order

```
tsc -b                              # Type-check frontend
  → vite build                      # Bundle frontend
  → scripts/build-native.cjs        # Compile native addon for Electron ABI
  → tsc -p electron/tsconfig.json   # Compile Electron main process
  → scripts/copy-native.cjs         # Copy .node to dist-electron/
  → scripts/set-cjs.cjs             # Write dist-electron/package.json
  → electron-builder                # Package AppImage
```

## Linting and type-checking

```bash
npm run lint           # ESLint (flat config, v9)
npx tsc -b             # TypeScript project references check
npx tsc --noEmit       # Type-check without emitting
```

## Native addon (Linux DnD)

The native addon at `native/src/linux_dnd.cc` replaces Electron's broken `startDrag()` on Wayland.

- **Stack**: GDK4 (GTK4's low-level library), N-API (`node-addon-api`)
- **Fallback**: If the addon fails to load (non-Linux, no GTK4, build error), the app falls back to Electron's built-in `event.sender.startDrag()`
- **Build**: Compiled against Electron's Node.js headers (must match the Electron version in `package.json`). The system's `node-gyp` handles this automatically via `scripts/build-native.cjs`

### Debugging the native addon

```bash
# Rebuild manually for Electron
node scripts/build-native.cjs

# Test in isolation (system Node, not Electron)
cd native
node -e "const m = require('./build/Release/linux_dnd.node'); console.log(m.init())"
```

## Architecture notes

- **Frontend**: React 19 + Vite, ESM. `verbatimModuleSyntax: true`, `noUnusedLocals` enabled
- **Backend**: Electron main process, CommonJS. Separate `electron/tsconfig.json`
- **IPC**: `contextBridge` in preload.ts, fire-and-forget `send()` or bidirectional `invoke()`
- **UI**: `@material/web` (Lit-based Material 3 web components)
- **CSS**: Plain `.css` files, no CSS-in-JS
- **No router**: Tab state managed in `App.tsx` via `useState`
- **File ops**: Linux system commands (`du`, `find`, `unzip`, `tar`, `xdg-mime`, etc.) — not portable to macOS/Windows

### Key conventions

- Always write JSDoc for public APIs
- Use 2-space indentation (enforced by ESLint)
- Import `@material/web` components in `src/material-web.ts`
- Type-only imports use `import type` (frontend only, due to `verbatimModuleSyntax`)
- Check for regressions before committing — many subsystems are tightly coupled
