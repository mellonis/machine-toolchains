const fs = require('fs'), path = require('path');
const src = path.join(__dirname, '..', '..', 'grammars', 'pmc.tmLanguage.json');
const dstDir = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(dstDir, { recursive: true });
fs.copyFileSync(src, path.join(dstDir, 'pmc.tmLanguage.json'));
