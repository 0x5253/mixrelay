import { copyFile, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import { parseArgs } from 'node:util';
import { build } from 'esbuild';
import { productVersion } from '../../scripts/product-version.mjs';
import { generateIcons } from './icons.mjs';

const { values } = parseArgs({ options: { output: { type: 'string' } } });
const output = values.output ?? 'dist/com.rawsector.mixrelay.sdPlugin';
if (basename(output) !== 'com.rawsector.mixrelay.sdPlugin') {
  throw new Error('--output must name com.rawsector.mixrelay.sdPlugin');
}
await rm(output, { recursive: true, force: true });
await mkdir(`${output}/images`, { recursive: true });
for (const action of ['power', 'mute', 'volume', 'recording']) {
  await copyFile(`images/${action}.svg`, `${output}/images/${action}.svg`);
  const icon = (await readFile(`images/${action}.svg`, 'utf8'))
    .replace('<svg ', '<svg x="26" y="3" ');
  await writeFile(
    `${output}/images/key-${action}.svg`,
    `<svg xmlns="http://www.w3.org/2000/svg" width="72" height="72" viewBox="0 0 72 72"><rect width="72" height="72" fill="#191e20"/>${icon}</svg>`,
  );
}
await build({
  entryPoints: ['src/plugin.mjs'],
  outfile: `${output}/bin/plugin.js`,
  bundle: true,
  platform: 'node',
  target: 'node24',
  format: 'esm',
  banner: {
    js: "import { createRequire } from 'node:module'; const require = createRequire(import.meta.url);",
  },
});
const manifest = JSON.parse(await readFile('manifest.json', 'utf8'));
manifest.Version = `${productVersion()}.0`;
await writeFile(`${output}/manifest.json`, JSON.stringify(manifest, null, 2) + '\n');
await copyFile('ko.json', `${output}/ko.json`);
await copyFile('.sdignore', `${output}/.sdignore`);
await writeFile(`${output}/package.json`, '{"type":"module"}\n');
await cp('ui', `${output}/ui`, { recursive: true });
await build({
  entryPoints: ['ui/inspector.js'],
  outfile: `${output}/ui/inspector.js`,
  bundle: true,
  platform: 'browser',
  format: 'iife',
});
await generateIcons(output, join(dirname(output), 'marketplace'));
