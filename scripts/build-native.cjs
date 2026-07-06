/**
 * build-native.cjs — Build the GDK4 drag-and-drop native addon for Electron.
 *
 * Must be compiled against Electron's Node.js ABI (not the system Node.js),
 * otherwise the addon will crash at runtime with ABI mismatch.
 *
 * Called automatically by `npm run electron:dev` and `npm run build`.
 * On non-Linux systems or when GTK4 is not installed, this exits silently.
 */

const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');

const ROOT = path.join(__dirname, '..');
const NATIVE_DIR = path.join(ROOT, 'native');

// ── Platform guard ────────────────────────────────────────────────
if (process.platform !== 'linux') {
  console.log('[build-native] Skipping — not Linux');
  process.exit(0);
}

// ── GTK4 check ────────────────────────────────────────────────────
try {
  execSync('pkg-config --exists gtk4', { stdio: 'ignore' });
} catch {
  console.log('[build-native] GTK4 not found — skipping native addon (DnD will use Electron built-in)');
  process.exit(0);
}

// ── Electron version ──────────────────────────────────────────────
let electronVersion;
try {
  const electronPkgPath = path.join(ROOT, 'node_modules', 'electron', 'package.json');
  electronVersion = require(electronPkgPath).version;
} catch {
  console.error('[build-native] Cannot find Electron package — skipping');
  process.exit(0);
}

console.log(`[build-native] Building native addon for Electron ${electronVersion}…`);

// ── Resolve node-gyp ──────────────────────────────────────────────
let nodeGypBin;
const candidates = [
  path.join(ROOT, 'node_modules', '.bin', 'node-gyp'),
  path.join(NATIVE_DIR, 'node_modules', '.bin', 'node-gyp'),
  '/usr/lib/node_modules_22/npm/node_modules/node-gyp/bin/node-gyp.js',
];
for (const c of candidates) {
  if (fs.existsSync(c)) {
    nodeGypBin = c;
    break;
  }
}

if (!nodeGypBin) {
  console.error('[build-native] node-gyp not found — skipping native addon (DnD will use Electron built-in)');
  process.exit(0);
}

// ── Build ─────────────────────────────────────────────────────────
const electronDistUrl = 'https://electronjs.org/headers';
const nodeGypCmd = `node "${nodeGypBin}" rebuild --target=${electronVersion} --dist-url=${electronDistUrl}`;

try {
  execSync(nodeGypCmd, { cwd: NATIVE_DIR, stdio: 'inherit' });
  console.log('[build-native] ✓ Build successful');
} catch (err) {
  console.error('[build-native] Build failed (DnD will fall back to Electron built-in):', err.message);
  // Don't fail the whole build — native DnD is optional
  process.exit(0);
}
