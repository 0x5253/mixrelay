import { MINIMUM_MACOS } from './macos-minimum.mjs';
import { UNINSTALLER_BUNDLE_NAME } from "./macos-install-contract.mjs";
import * as fs from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL, fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const languages = JSON.parse(fs.readFileSync(join(root, 'packaging/localization/languages.json')));
const labels = JSON.parse(fs.readFileSync(join(root, 'packaging/localization/driver-removal.json')));
function command(program, args) {
  const result = spawnSync(program, args, {encoding:'utf8', timeout:120000});
  if (result.status !== 0 || result.error) throw new Error(`${program}: ${result.stderr || result.error?.message}`);
  return result.stdout.trim();
}
export function buildRemovalApp({executable, helper, output, version, identity}) {
  const team = identity.match(/^Developer ID Application: [^\r\n]+ \(([A-Z0-9]{10})\)$/)?.[1];
  if (!team || !/^\d+(?:\.\d+){1,3}$/.test(version)) throw new Error('Invalid release identity or version');
  for (const path of [executable, helper, output]) {
    if (!path.startsWith('/') || resolve(path) !== path) throw new Error('Release paths must be absolute');
    let current = '/';
    for (const component of path.split('/').filter(Boolean)) {
      current = join(current, component);
      if (!fs.existsSync(current)) break;
      if (fs.lstatSync(current).isSymbolicLink()) throw new Error('Release paths cannot contain symlinks');
    }
  }
  for (const path of [executable,helper]) {
    const arches = command('/usr/bin/lipo', ['-archs',path]).split(/\s+/).sort();
    if (arches.join(' ') !== 'arm64 x86_64') throw new Error('Removal executables must be Universal 2');
  }
  command('/usr/bin/codesign', ['--verify','--strict','--all-architectures',helper]);
  fs.mkdirSync(dirname(output), {recursive:true});
  const work = fs.mkdtempSync(join(dirname(output),'.mixrelay-remover-build-'));
  const app = join(work, UNINSTALLER_BUNDLE_NAME);
  let previous;
  try {
    for (const directory of ['MacOS','Helpers','Resources']) fs.mkdirSync(join(app,'Contents',directory), {recursive:true,mode:0o755});
    fs.copyFileSync(executable,join(app,'Contents/MacOS/mixrelay-driver-uninstaller'));
    fs.copyFileSync(helper,join(app,'Contents/Helpers/mixrelay-macos-installer'));
    for (const path of ['MacOS/mixrelay-driver-uninstaller','Helpers/mixrelay-macos-installer']) fs.chmodSync(join(app,'Contents',path),0o755);
    // Same tile as the app, symbol in brand ink: branding/build-uninstaller-icon.mjs.
    fs.copyFileSync(join(root,'apps/desktop/src-tauri/icons/uninstaller.icns'),join(app,'Contents/Resources/icon.icns'));
    fs.copyFileSync(join(root,'LICENSE.txt'),join(app,'Contents/Resources/LICENSE.txt'));
    for (const language of languages) {
      const localized = join(app,'Contents/Resources',`${language.language}.lproj`);
      fs.mkdirSync(localized,{mode:0o755});
      const title = labels[language.language].remove;
      fs.writeFileSync(join(localized,'InfoPlist.strings'),`"CFBundleDisplayName" = ${JSON.stringify(title)};\n`);
      fs.writeFileSync(join(localized,'Authorization.strings'),`"Remove MixRelay" = ${JSON.stringify(title)};\n`);
    }
    const info = {
      CFBundleIdentifier:'io.mixrelay.driver.uninstaller', CFBundleName:'Uninstall MixRelay',
      CFBundleDisplayName:'Uninstall MixRelay',CFBundleExecutable:'mixrelay-driver-uninstaller',
      CFBundlePackageType:'APPL',CFBundleShortVersionString:version,CFBundleVersion:version,
      CFBundleIconFile:'icon.icns',CFBundleDevelopmentRegion:'en',CFBundleLocalizations:languages.map(l=>l.language),
      LSMinimumSystemVersion:MINIMUM_MACOS,NSHighResolutionCapable:true,MixRelaySigningTeam:team,
    };
    const plist = join(app,'Contents/Info.plist');
    fs.writeFileSync(plist,JSON.stringify(info));
    command('/usr/bin/plutil',['-convert','xml1',plist]);
    command('/usr/bin/codesign',['--force','--sign',identity,'--options','runtime','--timestamp','--identifier','io.mixrelay.driver.uninstaller',app]);
    command('/usr/bin/codesign',['--verify','--deep','--strict','--all-architectures',app]);
    if (fs.existsSync(output)) { previous=join(work,'previous.app'); fs.renameSync(output,previous); }
    try { fs.renameSync(app,output); }
    catch (error) { if(previous) fs.renameSync(previous,output); throw error; }
    return output;
  } finally { fs.rmSync(work,{recursive:true,force:true}); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [executable, helper, output, version, identity] = process.argv.slice(2);
  console.log(buildRemovalApp({executable,helper,output,version,identity}));
}
