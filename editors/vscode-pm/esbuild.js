// Bundles the extension entry into a single CJS file so the vsix ships
// one script instead of the whole language-client dependency tree.
// `vscode` is provided by the host and must stay external.
const esbuild = require('esbuild');

esbuild.build({
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'out/extension.js',
  platform: 'node',
  format: 'cjs',
  target: 'node18',
  external: ['vscode'],
  minify: true,
}).catch(() => process.exit(1));
