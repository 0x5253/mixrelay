import { copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { MINIMUM_MACOS } from '../scripts/macos-minimum.mjs';
import { canonicalIcns } from './canonical-icns.mjs';

if (process.platform === 'darwin') {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const selected = spawnSync('xcode-select', ['-p'], { encoding: 'utf8' }).stdout?.trim();
  const developer = process.env.DEVELOPER_DIR
    || (selected?.includes('.app/Contents/Developer') ? selected : '/Applications/Xcode.app/Contents/Developer');
  const compiler = join(developer, 'usr/bin/actool');
  if (!existsSync(compiler)) throw new Error('Icon Composer assets require full Xcode; set DEVELOPER_DIR.');
  const output = join(root, 'target/app-icon/macos');
  mkdirSync(output, { recursive: true });
  const result = spawnSync(compiler, [
    join(root, 'branding/macos/MixRelay.icon'),
    '--compile', output,
    '--output-format', 'human-readable-text', '--notices', '--warnings',
    '--output-partial-info-plist', join(output, 'icon-info.plist'),
    '--app-icon', 'MixRelay', '--enable-on-demand-resources', 'NO',
    '--development-region', 'en', '--target-device', 'mac',
    '--minimum-deployment-target', MINIMUM_MACOS, '--platform', 'macosx',
  ], { env: { ...process.env, DEVELOPER_DIR: developer }, stdio: 'inherit' });
  if (result.error || result.status !== 0) throw new Error(result.error?.message || 'Apple icon compilation failed.');
  if (!existsSync(join(output, 'Assets.car'))) throw new Error('Apple icon asset catalog was not produced.');
  const icon = join(root, 'apps/desktop/src-tauri/icons/icon.icns');
  copyFileSync(join(output, 'MixRelay.icns'), icon);
  canonicalIcns(icon);
}
