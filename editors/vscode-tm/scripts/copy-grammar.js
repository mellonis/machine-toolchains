const fs = require('fs'), path = require('path');
const grammarsDir = path.join(__dirname, '..', '..', 'grammars');
const dstDir = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(dstDir, { recursive: true });
for (const name of ['tmc.tmLanguage.json', 'tma.tmLanguage.json']) {
  fs.copyFileSync(path.join(grammarsDir, name), path.join(dstDir, name));
}
