// Copies the single-source editor assets into this extension's package
// directory: the shared TextMate grammars and the shared JSON Schema.
// Both live one level up so the -pm and -tm pairs consume one copy.
const fs = require('fs'), path = require('path');
const editorsDir = path.join(__dirname, '..', '..');

const grammarsSrc = path.join(editorsDir, 'grammars');
const grammarsDst = path.join(__dirname, '..', 'syntaxes');
fs.mkdirSync(grammarsDst, { recursive: true });
for (const name of ['pmc.tmLanguage.json', 'pma.tmLanguage.json']) {
  fs.copyFileSync(path.join(grammarsSrc, name), path.join(grammarsDst, name));
}

const schemasSrc = path.join(editorsDir, 'schemas');
const schemasDst = path.join(__dirname, '..', 'schemas');
fs.mkdirSync(schemasDst, { recursive: true });
fs.copyFileSync(
  path.join(schemasSrc, 'pmt.schema.json'),
  path.join(schemasDst, 'pmt.schema.json'),
);
