/**
 * copy-native.cjs — Copy the compiled .node file into dist-electron/
 * so it gets bundled by electron-builder and is loadable at runtime.
 */

const fs = require('fs');
const path = require('path');

if (process.platform !== 'linux') {
  process.exit(0);
}

const src = path.join(__dirname, '..', 'native', 'build', 'Release', 'linux_dnd.node');
const dest = path.join(__dirname, '..', 'dist-electron', 'linux_dnd.node');

if (fs.existsSync(src)) {
  const destDir = path.dirname(dest);
  if (!fs.existsSync(destDir)) {
    fs.mkdirSync(destDir, { recursive: true });
  }
  fs.copyFileSync(src, dest);
  console.log('[copy-native] ✓ Copied linux_dnd.node → dist-electron/');
} else {
  console.log('[copy-native] linux_dnd.node not found — native DnD unavailable');
}
