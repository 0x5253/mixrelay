import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

if (process.platform === 'darwin') test('application discovery reads bundle PNGs without calling IconServices, including missing icons', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-icons-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'discovery');
  execFileSync('xcrun', ['clang', '-fobjc-arc', '-fblocks', '-Wall', '-Wextra', '-Werror',
    '-I', 'native/macos/application-capture/include',
    'native/macos/application-capture/tests/MRApplicationDiscoveryFixture.m',
    'native/macos/application-capture/src/MRProcessTap.m',
    '-framework', 'AppKit', '-framework', 'Foundation', '-framework', 'CoreAudio', '-o', binary],
  { timeout: 30000 });
  execFileSync(binary, [directory], { timeout: 10000 });
});
