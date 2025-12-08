const fs = require('node:fs');
const path = require('node:path');

const targetDir = path.join(__dirname, '../dist-electron');
if (!fs.existsSync(targetDir)) {
    fs.mkdirSync(targetDir, { recursive: true });
}

fs.writeFileSync(
    path.join(targetDir, 'package.json'),
    JSON.stringify({ type: 'commonjs' }, null, 2)
);
console.log('Created dist-electron/package.json with commonjs type.');
