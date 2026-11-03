// Builds the uninstaller's icon from the app icon: the same tile with the symbol in the
// brand ink instead of the brand red, so the two apps in /Applications/MixRelay differ at
// a glance. Run after changing identity/app-icon.svg:  node branding/build-uninstaller-icon.mjs
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { canonicalIcns } from './canonical-icns.mjs';

const root = path.dirname(fileURLToPath(import.meta.url));
const repository = path.dirname(root);
const tokens = fs.readFileSync(path.join(repository, 'apps/control-ui/src/core/theme/tokens.ts'), 'utf8');
// The approved brand colours (brandColors), not the UI palette, own brand assets.
const brand = tokens.slice(tokens.indexOf('export const brandColors'));
const ink = brand.match(/\bink: '(#[0-9a-f]{6})'/i)?.[1];
const paper = brand.match(/\bpaper: '(#[0-9a-f]{6})'/i)?.[1];
if (!ink || !paper) throw new Error('brandColors must define ink and paper in tokens.ts');
const appIcon = fs.readFileSync(path.join(root, 'identity/app-icon.svg'), 'utf8');
// The tile is paper; the symbol is the one other fill.
const symbolFills = [...new Set([...appIcon.matchAll(/fill="(#[0-9a-f]{6})"/gi)].map(match => match[1].toLowerCase()))]
  .filter(fill => fill !== paper.toLowerCase());
if (symbolFills.length !== 1) throw new Error(`app-icon.svg must have one symbol fill besides paper, found ${symbolFills.join(', ') || 'none'}`);
const red = symbolFills[0];
const work = fs.mkdtempSync(path.join(os.tmpdir(), 'mixrelay-uninstaller-icon-'));
try {
  const source = path.join(work, 'uninstaller-icon.svg');
  fs.writeFileSync(source, appIcon.replace(new RegExp(`fill="${red}"`, 'gi'), `fill="${ink}"`));
  const result = spawnSync('pnpm', ['--filter', '@mixrelay/desktop', 'tauri', 'icon', source, '--output', path.join(work, 'icons')],
    { cwd: repository, stdio: 'pipe' });
  if (result.error || result.status !== 0) throw new Error(result.error?.message || result.stderr.toString());
  canonicalIcns(path.join(work, 'icons/icon.icns'));
  fs.copyFileSync(path.join(work, 'icons/icon.icns'), path.join(repository, 'apps/desktop/src-tauri/icons/uninstaller.icns'));
} finally {
  fs.rmSync(work, { recursive: true, force: true });
}
console.log('apps/desktop/src-tauri/icons/uninstaller.icns');
