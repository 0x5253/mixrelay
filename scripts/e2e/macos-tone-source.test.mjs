import { MINIMUM_MACOS } from '../macos-minimum.mjs';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

if (process.platform === 'darwin') test('owned tone resumes a stopped configured engine only on a new notification and its exact device', t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-tone-recovery-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'recovery');
  execFileSync('xcrun', ['clang', '-fobjc-arc', '-fblocks', `-mmacosx-version-min=${MINIMUM_MACOS}`,
    '-Wall', '-Wextra', '-Werror', 'native/macos/application-capture/tests/MRVirtualToneRecoveryTests.m',
    '-framework', 'Foundation', '-framework', 'AVFAudio', '-framework', 'AudioUnit', '-framework', 'CoreAudio', '-o', binary],
  { timeout: 30000 });
  execFileSync(binary, { timeout: 10000 });
});
