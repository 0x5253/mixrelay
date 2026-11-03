import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// Only approved graphics enter this portable kit. Wordmark studies stay separate.
const root = path.dirname(fileURLToPath(import.meta.url));
const identity = path.join(root, 'identity');
const kit = path.join(identity, 'kit');
fs.mkdirSync(kit, { recursive: true });
for (const file of fs.readdirSync(path.join(identity, 'symbols')).filter(f => f.endsWith('.svg'))) {
  const name = file.slice(0, -4);
  const sizes = name.includes('outline') ? [64, 128, 256, 512] :
    name.includes('knockout') ? [32, 64, 128, 256, 512] : [16, 24, 32, 64, 128, 256, 512, 1024];
  const output = path.join(kit, 'png', name);
  const result = spawnSync('pnpm', ['--filter', '@mixrelay/desktop', 'tauri', 'icon',
    path.join(identity, 'symbols', file), '--output', output,
    ...sizes.flatMap(size => ['--png', String(size)])], { cwd: path.dirname(root), stdio: 'pipe' });
  if (result.error || result.status !== 0) throw new Error(result.error?.message || result.stderr.toString());
  fs.mkdirSync(path.join(kit, 'svg'), { recursive: true });
  fs.copyFileSync(path.join(identity, 'symbols', file), path.join(kit, 'svg', file));
}
fs.copyFileSync(path.join(identity, 'app-icon.svg'), path.join(kit, 'svg/app-icon.svg'));
fs.copyFileSync(path.join(root, 'tray-white.svg'), path.join(kit, 'svg/tray-white.svg'));
fs.mkdirSync(path.join(kit, 'app'), { recursive: true });
for (const file of ['32x32.png', '64x64.png', '128x128.png', '128x128@2x.png', 'icon.png', 'icon.icns', 'icon.ico']) {
  fs.copyFileSync(path.join(root, '../apps/desktop/src-tauri/icons', file), path.join(kit, 'app', file));
}
fs.cpSync(path.join(root, '../apps/desktop/src-tauri/tray-icons'), path.join(kit, 'tray'), { recursive: true });
for (const file of ['mixrelay-brand-system.svg']) {
  fs.copyFileSync(path.join(identity, file), path.join(kit, file));
}
for (const file of ['mixrelay-symbol-kit.ai', 'preview.png']) {
  if (fs.existsSync(path.join(identity, file))) fs.copyFileSync(path.join(identity, file), path.join(kit, file));
}
fs.mkdirSync(path.join(kit, 'source'), { recursive: true });
fs.copyFileSync(path.join(identity, 'source/symbol.json'), path.join(kit, 'source/symbol.json'));
const manifest = JSON.parse(fs.readFileSync(path.join(identity, 'manifest.json'), 'utf8'));
manifest.colors = 'colorValues';
delete manifest.research;
fs.writeFileSync(path.join(kit, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
fs.writeFileSync(path.join(kit, 'README.md'), `# MixRelay approved graphic kit v1.1

Symbol kit approved 2026-10-11: Optical Grid. Frozen Optical wordmark assets (2026-10-12) are provided separately in identity/wordmark; this ZIP remains the symbol kit.

- svg/: 10 filled, outlined and knockout symbols, app and tray masters.
- png/: transparent PNGs. Filled 16/24/32/64/128/256/512/1024; outline 64/128/256/512; knockout 32/64/128/256/512.
- app/: application PNG, ICNS and ICO. tray/: system template PNGs.
- source/symbol.json: approved geometry. manifest.json: actual color values and minimum sizes.
- mixrelay-brand-system.svg, mixrelay-symbol-kit.ai, preview.png: editable graphic guide and preview.

Clear space: 0.2 times silhouette height. Filled minimum canvas 16px; outline 64px; knockout 32px.
Do not stretch, mirror, add shadows or change stroke weight. Inverse means light on dark, not mirrored geometry.
Knockout is a real transparent hole. macOS owns template tray display colors.
Ordinary product-name text and accessibility labels are not a typographic wordmark.
`);
const zipPath = path.join(identity, 'mixrelay-symbol-kit.zip');
fs.rmSync(zipPath, { force: true });
const archive = spawnSync('zip', ['-q', '-r', zipPath, '.'], { cwd: kit, stdio: 'pipe' });
if (archive.error || archive.status !== 0) throw new Error(archive.error?.message || archive.stderr.toString());
console.log('Approved symbol kit exported: SVG, transparent PNG, app and tray assets, ZIP.');
