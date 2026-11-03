import { MINIMUM_MACOS } from '../macos-minimum.mjs';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export function prepareSourceComparison(directory, execute = execFileSync) {
  const bundle = resolve(directory, 'io.mixrelay.qualification.source-control.app');
  const executable = join(bundle, 'Contents/MacOS/MRSourceControl');
  mkdirSync(join(bundle, 'Contents/MacOS'), { recursive: true });
  execute('clang', ['-std=c17', '-O2', `-mmacosx-version-min=${MINIMUM_MACOS}`, '-Wall', '-Wextra', '-Wpedantic',
    '-Wconversion', '-Wsign-conversion', '-Wshadow', '-Werror', '-fblocks',
    ...['control', 'device', 'pcm'].map(name => `scripts/e2e/macos-source-${name}.c`),
    'native/macos/service/src/MRMicrophonePermission.m',
    ...['Activity.m', 'HalTrace.c', 'Authorization.c'].map(name => `native/macos/hal-driver/tests/MRProbe${name}`),
    ...['CoreAudio', 'CoreFoundation', 'AVFoundation', 'Security', 'Foundation'].flatMap(name => ['-framework', name]),
    '-o', executable], { timeout: 60000 });
  execute(executable, ['--self-test'], { timeout: 5000 });
  writeFileSync(join(bundle, 'Contents/Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.mixrelay.qualification.source-control</string>
<key>CFBundleExecutable</key><string>MRSourceControl</string>
<key>CFBundleName</key><string>MixRelay Source Control</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSMicrophoneUsageDescription</key><string>Compare silent and audible inputs on the MixRelay test device.</string>
</dict></plist>\n`, { flag: 'wx' });
  execute('codesign', ['--force', '--sign', '-', bundle], { timeout: 30000 });
  return bundle;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const usage = 'node scripts/e2e/prepare-source-comparison.mjs EMPTY_DIRECTORY\nCompiles an ad-hoc signed diagnostic app and runs its device-free callback self-test. Installed source comparisons run only through the focused candidate-e2e workflow.';
  if (process.argv[2] === '--help') console.log(usage);
  else {
    if (process.argv.length !== 3) throw new Error(usage);
    console.log(prepareSourceComparison(process.argv[2]));
  }
}
