const fs = require('fs'), path = require('path');
const grammarsDir = path.join(__dirname, '..', '..', 'grammars');
const dstDir = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(dstDir, { recursive: true });
for (const name of ['pmc.tmLanguage.json', 'pma.tmLanguage.json']) {
  fs.copyFileSync(path.join(grammarsDir, name), path.join(dstDir, name));
}
