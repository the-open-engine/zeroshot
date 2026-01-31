const fs = require('fs');
const path = require('path');

const srcPath = path.resolve(__dirname, '..', 'src', 'tui', 'package.json');
const destDir = path.resolve(__dirname, '..', 'lib', 'tui');
const destPath = path.join(destDir, 'package.json');

fs.mkdirSync(destDir, { recursive: true });
fs.copyFileSync(srcPath, destPath);
