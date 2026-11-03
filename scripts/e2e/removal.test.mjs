import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { removeInstalled } from './removal.mjs';

function fixture(t) {
  const output = mkdtempSync(join(tmpdir(), 'mixrelay-removal-'));
  t.after(() => rmSync(output, { recursive: true, force: true }));
  return output;
}

test('DEB and Arch removal use their package manager, reload services and require absence', (t) => {
  for (const format of ['deb', 'arch']) {
    const calls = [],
      output = fixture(t);
    removeInstalled(
      'linux',
      format,
      output,
      (program, args) => {
        calls.push([program, args]);
        return 'removed';
      },
      () => false,
    );
    assert.deepEqual(calls[0], [
      'sudo',
      format === 'deb'
        ? ['-n', 'apt-get', 'remove', '-y', 'mix-relay']
        : ['-n', 'pacman', '-R', '--noconfirm', 'mix-relay'],
    ]);
    assert.deepEqual(calls[1], ['systemctl', ['--user', 'daemon-reload']]);
    assert.match(readFileSync(join(output, 'removal.log'), 'utf8'), /removed/);
  }
  assert.throws(
    () =>
      removeInstalled(
        'linux',
        'deb',
        fixture(t),
        () => 'done',
        () => true,
      ),
    /left installed/,
  );
});

test('macOS copies the signed native transaction helper outside the removed product and preserves user data', (t) => {
  const calls = [],
    output = fixture(t);
  removeInstalled(
    'macos',
    'pkg',
    output,
    (program, args) => {
      calls.push([program, args]);
      return program === 'codesign' ? '\nTeamIdentifier=ABCDE12345\n' : 'done';
    },
    () => false,
  );
  const removal = calls.find(([program]) => program === 'sudo')[1];
  assert.ok(!removal[1].startsWith('/Applications/'));
  assert.deepEqual(removal.slice(2), ['remove', 'ABCDE12345', '--reload-coreaudio']);
  assert.ok(calls.some(([program, args]) => program === 'codesign' && args[0] === '--verify'));
  assert.throws(
    () =>
      removeInstalled(
        'macos',
        'pkg',
        fixture(t),
        () => 'unsigned',
        () => false,
      ),
    /publisher/,
  );
});

test('a failed native removal remains fatal and preserves the error log', (t) => {
  const output = fixture(t);
  assert.throws(
    () =>
      removeInstalled(
        'linux',
        'arch',
        output,
        () => {
          throw new Error('active processes rejected removal');
        },
        () => false,
      ),
    /active processes/,
  );
  assert.match(
    readFileSync(join(output, 'removal.log'), 'utf8'),
    /active processes rejected removal/,
  );
});

test('removal fails while an executable from before the engine rename remains', (t) => {
  for (const [os, format, stale] of [
    ['macos', 'pkg', '/Library/Application Support/MixRelay/bin/mixrelay-engine-service'],
    ['linux', 'deb', '/usr/bin/mixrelay-engine-service'],
  ]) {
    assert.throws(
      () =>
        removeInstalled(
          os,
          format,
          fixture(t),
          (program) => (program === 'codesign' ? '\nTeamIdentifier=ABCDE12345\n' : 'done'),
          (path) => path === stale,
        ),
      (error) => error.message.endsWith(`left installed product artifacts: ${stale}`),
    );
  }
});
